use ingot_domain::item::WorkflowVersion;
use ingot_domain::job::PhaseKind;
use ingot_domain::step_id::StepId;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct WorkflowStepPresentation {
    pub id: StepId,
    pub label: &'static str,
    pub phase: PhaseKind,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct WorkflowPhasePresentation {
    pub id: &'static str,
    pub label: &'static str,
    pub steps: &'static [WorkflowStepPresentation],
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct WorkflowFindingsCopy {
    pub agent_scope_title: &'static str,
    pub current_section_title: &'static str,
    pub current_section_hint: &'static str,
    pub previous_section_title: &'static str,
    pub previous_section_summary_noun: &'static str,
    pub triage_warning: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct WorkflowPresentation {
    pub version: WorkflowVersion,
    pub phases: &'static [WorkflowPhasePresentation],
    pub findings_copy: WorkflowFindingsCopy,
}

pub static DELIVERY_CANDIDATE_PHASE_STEPS: &[WorkflowStepPresentation] = &[
    WorkflowStepPresentation {
        id: StepId::AuthorInitial,
        label: "Author",
        phase: PhaseKind::Author,
    },
    WorkflowStepPresentation {
        id: StepId::ReviewIncrementalInitial,
        label: "Incr. Review",
        phase: PhaseKind::Review,
    },
    WorkflowStepPresentation {
        id: StepId::ReviewCandidateInitial,
        label: "Cand. Review",
        phase: PhaseKind::Review,
    },
    WorkflowStepPresentation {
        id: StepId::ValidateCandidateInitial,
        label: "Validate",
        phase: PhaseKind::Validate,
    },
    WorkflowStepPresentation {
        id: StepId::RepairCandidate,
        label: "Repair",
        phase: PhaseKind::Author,
    },
    WorkflowStepPresentation {
        id: StepId::ReviewIncrementalRepair,
        label: "Re-review",
        phase: PhaseKind::Review,
    },
    WorkflowStepPresentation {
        id: StepId::ReviewCandidateRepair,
        label: "Cand. Re-review",
        phase: PhaseKind::Review,
    },
    WorkflowStepPresentation {
        id: StepId::ValidateCandidateRepair,
        label: "Re-validate",
        phase: PhaseKind::Validate,
    },
    WorkflowStepPresentation {
        id: StepId::InvestigateItem,
        label: "Investigate",
        phase: PhaseKind::Investigate,
    },
];

pub static DELIVERY_CONVERGE_PHASE_STEPS: &[WorkflowStepPresentation] =
    &[WorkflowStepPresentation {
        id: StepId::PrepareConvergence,
        label: "Prepare",
        phase: PhaseKind::System,
    }];

pub static DELIVERY_INTEGRATION_PHASE_STEPS: &[WorkflowStepPresentation] = &[
    WorkflowStepPresentation {
        id: StepId::ValidateIntegrated,
        label: "Validate",
        phase: PhaseKind::Validate,
    },
    WorkflowStepPresentation {
        id: StepId::RepairAfterIntegration,
        label: "Repair",
        phase: PhaseKind::Author,
    },
    WorkflowStepPresentation {
        id: StepId::ReviewIncrementalAfterIntegrationRepair,
        label: "Incr. Review",
        phase: PhaseKind::Review,
    },
    WorkflowStepPresentation {
        id: StepId::ReviewAfterIntegrationRepair,
        label: "Cand. Review",
        phase: PhaseKind::Review,
    },
    WorkflowStepPresentation {
        id: StepId::ValidateAfterIntegrationRepair,
        label: "Re-validate",
        phase: PhaseKind::Validate,
    },
];

pub static INVESTIGATION_PHASE_STEPS: &[WorkflowStepPresentation] = &[
    WorkflowStepPresentation {
        id: StepId::InvestigateProject,
        label: "Investigate",
        phase: PhaseKind::Investigate,
    },
    WorkflowStepPresentation {
        id: StepId::ReinvestigateProject,
        label: "Reinvestigate",
        phase: PhaseKind::Investigate,
    },
];

pub static DELIVERY_PHASES: &[WorkflowPhasePresentation] = &[
    WorkflowPhasePresentation {
        id: "candidate",
        label: "Candidate",
        steps: DELIVERY_CANDIDATE_PHASE_STEPS,
    },
    WorkflowPhasePresentation {
        id: "converge",
        label: "Converge",
        steps: DELIVERY_CONVERGE_PHASE_STEPS,
    },
    WorkflowPhasePresentation {
        id: "integration",
        label: "Integration",
        steps: DELIVERY_INTEGRATION_PHASE_STEPS,
    },
];

pub static INVESTIGATION_PHASES: &[WorkflowPhasePresentation] = &[WorkflowPhasePresentation {
    id: "investigation",
    label: "Investigation",
    steps: INVESTIGATION_PHASE_STEPS,
}];

pub static WORKFLOW_PRESENTATIONS: &[WorkflowPresentation] = &[
    WorkflowPresentation {
        version: WorkflowVersion::DeliveryV1,
        phases: DELIVERY_PHASES,
        findings_copy: WorkflowFindingsCopy {
            agent_scope_title: "Agent scope for next repair job",
            current_section_title: "Current Review",
            current_section_hint: "agent acts on these findings only",
            previous_section_title: "Previous Reviews",
            previous_section_summary_noun: "earlier job",
            triage_warning: "Triage all findings before the agent can proceed.",
        },
    },
    WorkflowPresentation {
        version: WorkflowVersion::InvestigationV1,
        phases: INVESTIGATION_PHASES,
        findings_copy: WorkflowFindingsCopy {
            agent_scope_title: "Current investigation findings",
            current_section_title: "Current Investigation",
            current_section_hint: "triage or promote from this run",
            previous_section_title: "Previous Investigation Runs",
            previous_section_summary_noun: "earlier investigation run",
            triage_warning: "Triage all findings before the investigation can close.",
        },
    },
];

pub fn presentation_for_workflow(version: WorkflowVersion) -> &'static WorkflowPresentation {
    WORKFLOW_PRESENTATIONS
        .iter()
        .find(|presentation| presentation.version == version)
        .expect("all workflow versions must have presentation metadata")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DELIVERY_V1_STEPS, INVESTIGATION_V1_STEPS};

    #[test]
    fn delivery_presentation_steps_match_delivery_contract_order() {
        let presentation_steps = DELIVERY_PHASES
            .iter()
            .flat_map(|phase| phase.steps.iter().map(|step| step.id))
            .collect::<Vec<_>>();
        let contract_steps = DELIVERY_V1_STEPS
            .iter()
            .map(|step| step.step_id)
            .collect::<Vec<_>>();

        assert_eq!(presentation_steps, contract_steps);
    }

    #[test]
    fn investigation_presentation_steps_match_investigation_contract_order() {
        let presentation_steps = INVESTIGATION_PHASES
            .iter()
            .flat_map(|phase| phase.steps.iter().map(|step| step.id))
            .collect::<Vec<_>>();
        let contract_steps = INVESTIGATION_V1_STEPS
            .iter()
            .map(|step| step.step_id)
            .collect::<Vec<_>>();

        assert_eq!(presentation_steps, contract_steps);
    }
}
