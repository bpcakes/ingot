import type {
  WorkflowFindingsCopy,
  WorkflowPhasePresentation,
  WorkflowPresentation,
  WorkflowVersion,
} from '../../types/domain'

export type WorkflowStepDef = WorkflowPhasePresentation['steps'][number]
export type WorkflowPhaseDef = WorkflowPhasePresentation
export type { WorkflowFindingsCopy, WorkflowPresentation, WorkflowVersion }

const workflowVersions = ['delivery:v1', 'investigation:v1'] as const satisfies readonly WorkflowVersion[]

export type WorkflowPresentationLookup = Record<WorkflowVersion, WorkflowPresentation>

export function workflowPresentationLookup(catalog: WorkflowPresentation[]): WorkflowPresentationLookup {
  const lookup = Object.fromEntries(
    catalog.map((presentation) => [presentation.version, presentation]),
  ) as Partial<WorkflowPresentationLookup>

  for (const version of workflowVersions) {
    if (!lookup[version]) {
      throw new Error(`Server returned incomplete workflow presentation metadata for ${version}`)
    }
  }

  return lookup as WorkflowPresentationLookup
}
