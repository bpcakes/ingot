use ingot_domain::finding::FindingTriageState;
use ingot_domain::item::{Item, ParkingState};
use ingot_domain::job::OutcomeClass;
use ingot_domain::revision::ItemRevision;
use ingot_domain::step_id::StepId;

use crate::graph::{TransitionTarget, WorkflowGraph};
use crate::recommended_action::{NamedRecommendedAction, RecommendedAction};
use crate::step;

use super::{
    AllowedAction, AttentionBadge, BoardStatus, Evaluation, PhaseStatus, RevisionWorkflowSlice,
};

pub(super) fn evaluate_investigation(
    graph: &WorkflowGraph,
    item: &Item,
    _revision: &ItemRevision,
    slice: &RevisionWorkflowSlice<'_>,
    attention_badges: Vec<AttentionBadge>,
    mut diagnostics: Vec<String>,
) -> Evaluation {
    let active_job = slice.active_job();
    let latest_closure_job = slice.latest_closure_job();

    if item.escalation.is_escalated() {
        return finish(
            item,
            attention_badges,
            Evaluation {
                current_step_id: latest_closure_job.map(|j| j.step_id),
                next_recommended_action: RecommendedAction::named(
                    NamedRecommendedAction::OperatorIntervention,
                ),
                allowed_actions: vec![
                    AllowedAction::Revise,
                    AllowedAction::Dismiss,
                    AllowedAction::Invalidate,
                    AllowedAction::Defer,
                ],
                ..Evaluation::for_status(BoardStatus::Working, PhaseStatus::Escalated, diagnostics)
            },
        );
    }

    if item.parking_state == ParkingState::Deferred {
        return finish(
            item,
            attention_badges,
            Evaluation {
                current_step_id: latest_closure_job.map(|j| j.step_id),
                allowed_actions: vec![AllowedAction::Resume],
                ..Evaluation::for_status(BoardStatus::Inbox, PhaseStatus::Deferred, diagnostics)
            },
        );
    }

    if let Some(job) = active_job {
        return finish(
            item,
            attention_badges,
            Evaluation {
                current_step_id: Some(job.step_id),
                current_phase_kind: Some(job.phase_kind),
                allowed_actions: vec![AllowedAction::CancelJob],
                ..Evaluation::for_status(BoardStatus::Working, PhaseStatus::Running, diagnostics)
            },
        );
    }

    let Some(last_job) = latest_closure_job else {
        return finish(
            item,
            attention_badges,
            Evaluation {
                next_recommended_action: RecommendedAction::dispatch(StepId::InvestigateProject),
                dispatchable_step_id: Some(StepId::InvestigateProject),
                allowed_actions: vec![AllowedAction::Dispatch],
                ..Evaluation::for_status(BoardStatus::Inbox, PhaseStatus::New, diagnostics)
            },
        );
    };

    let Some(outcome) = last_job.state.outcome_class() else {
        diagnostics.push(format!(
            "investigation job {} has no outcome_class despite terminal status",
            last_job.step_id,
        ));
        return finish(
            item,
            attention_badges,
            Evaluation {
                current_step_id: Some(last_job.step_id),
                next_recommended_action: RecommendedAction::named(
                    NamedRecommendedAction::OperatorIntervention,
                ),
                ..Evaluation::for_status(BoardStatus::Working, PhaseStatus::Unknown, diagnostics)
            },
        );
    };

    if outcome == OutcomeClass::Clean {
        return finish(
            item,
            attention_badges,
            Evaluation {
                current_step_id: Some(last_job.step_id),
                terminal_readiness: true,
                ..Evaluation::for_status(BoardStatus::Working, PhaseStatus::Idle, diagnostics)
            },
        );
    }

    if outcome == OutcomeClass::Findings {
        let job_findings = slice
            .findings()
            .iter()
            .copied()
            .filter(|finding| finding.source_job_id == last_job.id)
            .collect::<Vec<_>>();

        if job_findings
            .iter()
            .any(|finding| finding.triage.is_unresolved())
        {
            return finish(
                item,
                attention_badges,
                Evaluation {
                    current_step_id: Some(last_job.step_id),
                    next_recommended_action: RecommendedAction::named(
                        NamedRecommendedAction::TriageFindings,
                    ),
                    ..Evaluation::for_status(
                        BoardStatus::Working,
                        PhaseStatus::Triaging,
                        diagnostics,
                    )
                },
            );
        }

        let has_actionable_findings = job_findings.iter().any(|finding| {
            matches!(
                finding.triage.state(),
                FindingTriageState::FixNow | FindingTriageState::NeedsInvestigation
            )
        });

        if has_actionable_findings {
            if let Some(next_step) = next_dispatchable_step(graph, last_job.step_id) {
                return finish(
                    item,
                    attention_badges,
                    Evaluation {
                        current_step_id: Some(last_job.step_id),
                        next_recommended_action: RecommendedAction::dispatch(next_step),
                        dispatchable_step_id: Some(next_step),
                        allowed_actions: vec![AllowedAction::Dispatch],
                        ..Evaluation::for_status(
                            BoardStatus::Working,
                            PhaseStatus::Idle,
                            diagnostics,
                        )
                    },
                );
            }
        }

        return finish(
            item,
            attention_badges,
            Evaluation {
                current_step_id: Some(last_job.step_id),
                terminal_readiness: true,
                ..Evaluation::for_status(BoardStatus::Working, PhaseStatus::Idle, diagnostics)
            },
        );
    }

    // Terminal/transient failure — needs operator intervention
    finish(
        item,
        attention_badges,
        Evaluation {
            current_step_id: Some(last_job.step_id),
            ..Evaluation::for_status(BoardStatus::Working, PhaseStatus::Idle, diagnostics)
        },
    )
}

fn next_dispatchable_step(graph: &WorkflowGraph, step_id: StepId) -> Option<StepId> {
    let Some(TransitionTarget::Step(next_step)) = graph.next_step(step_id, &OutcomeClass::Findings)
    else {
        return None;
    };

    step::find_step(*next_step)
        .is_dispatchable_job()
        .then_some(*next_step)
}

fn finish(
    item: &Item,
    attention_badges: Vec<AttentionBadge>,
    mut evaluation: Evaluation,
) -> Evaluation {
    evaluation.attention_badges = attention_badges;
    evaluation.board_status = if item.lifecycle.is_done() {
        BoardStatus::Done
    } else if evaluation.phase_status == Some(PhaseStatus::Running)
        || evaluation.dispatchable_step_id.is_some()
        || evaluation.terminal_readiness
    {
        BoardStatus::Working
    } else {
        evaluation.board_status
    };
    evaluation
}
