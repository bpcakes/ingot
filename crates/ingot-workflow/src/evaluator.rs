mod investigation;
mod projection;
#[cfg(test)]
mod tests;

use ingot_domain::convergence::{CheckoutAdoptionState, Convergence, ConvergenceStatus};
use ingot_domain::finding::Finding;
use ingot_domain::item::{ApprovalState, Item, ParkingState, WorkflowVersion};
use ingot_domain::job::{Job, PhaseKind};
use ingot_domain::revision::ItemRevision;
use ingot_domain::step_id::StepId;

use crate::graph::WorkflowGraph;
use crate::recommended_action::{NamedRecommendedAction, RecommendedAction};
use crate::step::{self, ClosureRelevance};

use self::projection::{
    auxiliary_steps, evaluate_idle_projection, latest_closure_terminal_job, merge_allowed_actions,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseStatus {
    New,
    Done,
    Running,
    Idle,
    Escalated,
    Deferred,
    PendingApproval,
    AwaitingConvergence,
    AwaitingCheckoutSync,
    Triaging,
    FinalizationReady,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllowedAction {
    Dispatch,
    CancelJob,
    ApprovalApprove,
    ApprovalReject,
    PrepareConvergence,
    Resume,
    Revise,
    Dismiss,
    Invalidate,
    Defer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionBadge {
    Escalated,
    Deferred,
}

/// Board column for UI rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BoardStatus {
    Inbox,
    Working,
    Approval,
    Done,
}

/// Pure read-side projection of item state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Evaluation {
    pub board_status: BoardStatus,
    pub attention_badges: Vec<AttentionBadge>,
    pub current_step_id: Option<StepId>,
    pub current_phase_kind: Option<PhaseKind>,
    pub phase_status: Option<PhaseStatus>,
    pub next_recommended_action: RecommendedAction,
    pub dispatchable_step_id: Option<StepId>,
    pub auxiliary_dispatchable_step_ids: Vec<StepId>,
    pub allowed_actions: Vec<AllowedAction>,
    pub terminal_readiness: bool,
    pub diagnostics: Vec<String>,
}

impl Evaluation {
    fn for_status(
        board_status: BoardStatus,
        phase_status: PhaseStatus,
        diagnostics: Vec<String>,
    ) -> Self {
        Self {
            board_status,
            attention_badges: Vec::new(),
            current_step_id: None,
            current_phase_kind: None,
            phase_status: Some(phase_status),
            next_recommended_action: RecommendedAction::None,
            dispatchable_step_id: None,
            auxiliary_dispatchable_step_ids: Vec::new(),
            allowed_actions: Vec::new(),
            terminal_readiness: false,
            diagnostics,
        }
    }
}

#[derive(Debug)]
struct RevisionWorkflowSlice<'a> {
    jobs: Vec<&'a Job>,
    findings: Vec<&'a Finding>,
    convergences: Vec<&'a Convergence>,
}

impl<'a> RevisionWorkflowSlice<'a> {
    fn for_current_revision(
        item: &Item,
        jobs: &'a [Job],
        findings: &'a [Finding],
        convergences: &'a [Convergence],
    ) -> Self {
        let revision_id = item.current_revision_id;

        Self {
            jobs: jobs
                .iter()
                .filter(|job| job.item_revision_id == revision_id)
                .collect(),
            findings: findings
                .iter()
                .filter(|finding| finding.source_item_revision_id == revision_id)
                .collect(),
            convergences: convergences
                .iter()
                .filter(|conv| conv.item_revision_id == revision_id)
                .collect(),
        }
    }

    fn findings(&self) -> &[&'a Finding] {
        &self.findings
    }

    fn active_job(&self) -> Option<&'a Job> {
        self.jobs.iter().copied().find(|job| job.state.is_active())
    }

    fn active_convergence(&self) -> Option<&'a Convergence> {
        self.convergences.iter().copied().find(|conv| {
            matches!(
                conv.state.status(),
                ConvergenceStatus::Queued | ConvergenceStatus::Running
            )
        })
    }

    fn prepared_convergence(&self) -> Option<&'a Convergence> {
        self.convergences
            .iter()
            .copied()
            .find(|conv| conv.state.status() == ConvergenceStatus::Prepared)
    }

    fn awaiting_checkout_sync(&self) -> Option<&'a Convergence> {
        self.convergences.iter().copied().find(|conv| {
            conv.state.status() == ConvergenceStatus::Finalized
                && matches!(
                    conv.state.checkout_adoption_state(),
                    Some(CheckoutAdoptionState::Pending | CheckoutAdoptionState::Blocked)
                )
        })
    }

    fn latest_closure_job(&self) -> Option<&'a Job> {
        latest_closure_terminal_job(&self.jobs)
    }
}

pub struct Evaluator {
    delivery_graph: WorkflowGraph,
    investigation_graph: WorkflowGraph,
}

impl Default for Evaluator {
    fn default() -> Self {
        Self::new()
    }
}

impl Evaluator {
    pub fn new() -> Self {
        Self {
            delivery_graph: WorkflowGraph::delivery_v1(),
            investigation_graph: WorkflowGraph::investigation_v1(),
        }
    }

    /// Evaluate the current state of an item.
    ///
    /// This is pure read-side logic. It MUST NOT mutate durable state.
    pub fn evaluate(
        &self,
        item: &Item,
        revision: &ItemRevision,
        jobs: &[Job],
        findings: &[Finding],
        convergences: &[Convergence],
    ) -> Evaluation {
        let mut diagnostics = Vec::new();
        let mut attention_badges = Vec::new();

        if item.escalation.is_escalated() {
            attention_badges.push(AttentionBadge::Escalated);
        }
        if item.parking_state == ParkingState::Deferred {
            attention_badges.push(AttentionBadge::Deferred);
        }

        if item.lifecycle.is_done() {
            return Evaluation {
                attention_badges,
                ..Evaluation::for_status(BoardStatus::Done, PhaseStatus::Done, diagnostics)
            };
        }

        let slice = RevisionWorkflowSlice::for_current_revision(item, jobs, findings, convergences);

        if item.workflow_version == WorkflowVersion::InvestigationV1 {
            return investigation::evaluate_investigation(
                &self.investigation_graph,
                item,
                revision,
                &slice,
                attention_badges,
                diagnostics,
            );
        }

        let active_job = slice.active_job();
        let active_convergence = slice.active_convergence();
        let prepared_convergence = slice.prepared_convergence();
        let awaiting_checkout_sync = slice.awaiting_checkout_sync();

        let latest_closure_job = slice.latest_closure_job();
        let has_terminal_closure_job = latest_closure_job.is_some();

        if awaiting_checkout_sync.is_some() {
            return self.finish_evaluation(
                item,
                has_terminal_closure_job,
                attention_badges,
                Evaluation {
                    current_step_id: Some(StepId::PrepareConvergence),
                    next_recommended_action: RecommendedAction::named(
                        NamedRecommendedAction::ResolveCheckoutSync,
                    ),
                    ..Evaluation::for_status(
                        BoardStatus::Working,
                        PhaseStatus::AwaitingCheckoutSync,
                        diagnostics,
                    )
                },
            );
        }

        if let Some(job) = active_job {
            let contract = step::find_step(job.step_id);
            let is_report_only = contract.closure_relevance == ClosureRelevance::ReportOnly;

            if is_report_only {
                let base = evaluate_idle_projection(
                    &self.delivery_graph,
                    item,
                    revision,
                    latest_closure_job,
                    slice.findings(),
                    prepared_convergence,
                    &mut diagnostics,
                );

                return self.finish_evaluation(
                    item,
                    has_terminal_closure_job,
                    attention_badges,
                    Evaluation {
                        current_step_id: base.current_step_id,
                        current_phase_kind: Some(job.phase_kind),
                        allowed_actions: vec![AllowedAction::CancelJob],
                        ..Evaluation::for_status(
                            BoardStatus::Working,
                            PhaseStatus::Running,
                            diagnostics,
                        )
                    },
                );
            }

            return self.finish_evaluation(
                item,
                has_terminal_closure_job,
                attention_badges,
                Evaluation {
                    current_step_id: Some(job.step_id),
                    current_phase_kind: Some(job.phase_kind),
                    allowed_actions: vec![AllowedAction::CancelJob],
                    ..Evaluation::for_status(
                        BoardStatus::Working,
                        PhaseStatus::Running,
                        diagnostics,
                    )
                },
            );
        }

        if active_convergence.is_some() {
            return self.finish_evaluation(
                item,
                has_terminal_closure_job,
                attention_badges,
                Evaluation {
                    current_step_id: Some(StepId::PrepareConvergence),
                    current_phase_kind: Some(PhaseKind::System),
                    ..Evaluation::for_status(
                        BoardStatus::Working,
                        PhaseStatus::Running,
                        diagnostics,
                    )
                },
            );
        }

        let base = evaluate_idle_projection(
            &self.delivery_graph,
            item,
            revision,
            latest_closure_job,
            slice.findings(),
            prepared_convergence,
            &mut diagnostics,
        );
        let auxiliary_dispatchable_step_ids =
            auxiliary_steps(item, &base.next_recommended_action, base.phase_status);
        let allowed_actions =
            merge_allowed_actions(base.allowed_actions, &auxiliary_dispatchable_step_ids);

        self.finish_evaluation(
            item,
            has_terminal_closure_job,
            attention_badges,
            Evaluation {
                current_step_id: base.current_step_id,
                next_recommended_action: base.next_recommended_action,
                dispatchable_step_id: base.dispatchable_step_id,
                auxiliary_dispatchable_step_ids,
                allowed_actions,
                terminal_readiness: base.terminal_readiness,
                ..Evaluation::for_status(BoardStatus::Working, base.phase_status, diagnostics)
            },
        )
    }

    fn finish_evaluation(
        &self,
        item: &Item,
        has_terminal_closure_job: bool,
        attention_badges: Vec<AttentionBadge>,
        mut evaluation: Evaluation,
    ) -> Evaluation {
        evaluation.attention_badges = attention_badges;

        evaluation.board_status = if item.lifecycle.is_done() {
            BoardStatus::Done
        } else if item.approval_state == ApprovalState::Pending
            && evaluation.next_recommended_action
                != RecommendedAction::named(NamedRecommendedAction::InvalidatePreparedConvergence)
        {
            BoardStatus::Approval
        } else if evaluation.phase_status == Some(PhaseStatus::Running) || has_terminal_closure_job
        {
            BoardStatus::Working
        } else {
            BoardStatus::Inbox
        };

        evaluation
    }
}
