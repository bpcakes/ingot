import { workflowPresentationLookup } from '../components/item-detail/workflowPresentation'
import type { WorkflowPresentation } from '../types/domain'

export const testWorkflowPresentations: WorkflowPresentation[] = [
  {
    version: 'delivery:v1',
    phases: [
      {
        id: 'candidate',
        label: 'Candidate',
        steps: [{ id: 'author_initial', label: 'Author', phase: 'author' }],
      },
      {
        id: 'converge',
        label: 'Converge',
        steps: [{ id: 'prepare_convergence', label: 'Prepare', phase: 'system' }],
      },
      {
        id: 'integration',
        label: 'Integration',
        steps: [{ id: 'validate_integrated', label: 'Validate', phase: 'validate' }],
      },
    ],
    findings_copy: {
      agent_scope_title: 'Agent scope for next repair job',
      current_section_title: 'Current Review',
      current_section_hint: 'agent acts on these findings only',
      previous_section_title: 'Previous Reviews',
      previous_section_summary_noun: 'earlier job',
      triage_warning: 'Triage all findings before the agent can proceed.',
    },
  },
  {
    version: 'investigation:v1',
    phases: [
      {
        id: 'investigation',
        label: 'Investigation',
        steps: [
          { id: 'investigate_project', label: 'Investigate', phase: 'investigate' },
          { id: 'reinvestigate_project', label: 'Reinvestigate', phase: 'investigate' },
        ],
      },
    ],
    findings_copy: {
      agent_scope_title: 'Current investigation findings',
      current_section_title: 'Current Investigation',
      current_section_hint: 'triage or promote from this run',
      previous_section_title: 'Previous Investigation Runs',
      previous_section_summary_noun: 'earlier investigation run',
      triage_warning: 'Triage all findings before the investigation can close.',
    },
  },
]

export const testWorkflowPresentationLookup = workflowPresentationLookup(testWorkflowPresentations)
