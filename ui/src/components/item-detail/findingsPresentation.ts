import type { Finding, FindingTriageState, Job, LinkedFindingItemSummary } from '../../types/domain'
import { WORKFLOW_FINDINGS_COPY, type WorkflowFindingsCopy, type WorkflowVersion } from './workflowPresentation'

export type FindingGroup = {
  jobId: string
  job: Job | undefined
  stepId: string
  findings: Finding[]
  isLatest: boolean
}

export type FindingActionMode = 'delivery' | 'investigation'

export type FindingTriageCopy = {
  mode: FindingActionMode
  fixNowLabel: string
  fixNowDescription: string
  quickFixNowLabel: string
  backlogDescription: string
  quickBacklogLabel?: string
}

export const DELIVERY_TRIAGE_COPY: FindingTriageCopy = {
  mode: 'delivery',
  fixNowLabel: 'Fix now',
  fixNowDescription: 'Agent will repair this finding',
  quickFixNowLabel: 'Fix now',
  backlogDescription: 'Promote to separate item',
}

export const INVESTIGATION_TRIAGE_COPY: FindingTriageCopy = {
  mode: 'investigation',
  fixNowLabel: 'Fix now',
  fixNowDescription: 'Create and launch a linked change item from this finding',
  quickFixNowLabel: 'Fix now',
  backlogDescription: 'Create a linked change item without launching it',
  quickBacklogLabel: 'Backlog',
}

export const NEEDS_NOTE: ReadonlySet<FindingTriageState> = new Set([
  'wont_fix',
  'dismissed_invalid',
  'needs_investigation',
])

export const NEEDS_LINK: ReadonlySet<FindingTriageState> = new Set(['backlog', 'duplicate'])

export function groupFindingsByJob(findings: Finding[], jobs: Job[]): FindingGroup[] {
  const jobMap = new Map(jobs.map((j) => [j.id, j]))
  const grouped = new Map<string, Finding[]>()

  for (const finding of findings) {
    const list = grouped.get(finding.source_job_id) ?? []
    list.push(finding)
    grouped.set(finding.source_job_id, list)
  }

  const groups: FindingGroup[] = []
  for (const [jobId, groupFindings] of grouped) {
    const job = jobMap.get(jobId)
    groups.push({
      jobId,
      job,
      stepId: groupFindings[0].source_step_id,
      findings: groupFindings,
      isLatest: false,
    })
  }

  groups.sort((a, b) => {
    const aTime = a.job?.ended_at ?? a.findings[0]?.created_at ?? ''
    const bTime = b.job?.ended_at ?? b.findings[0]?.created_at ?? ''
    return bTime.localeCompare(aTime)
  })

  if (groups.length > 0) {
    groups[0].isLatest = true
  }

  return groups
}

export function isInvestigationGroup(group: FindingGroup): boolean {
  return (
    group.job?.phase_kind === 'investigate' ||
    group.stepId === 'investigate_item' ||
    group.stepId === 'investigate_project' ||
    group.stepId === 'reinvestigate_project' ||
    group.findings.some((finding) => finding.investigation !== null)
  )
}

export function findingsCopyForGroup(
  group: FindingGroup | undefined,
  workflowVersion: WorkflowVersion,
): WorkflowFindingsCopy {
  if (group && isInvestigationGroup(group)) {
    return WORKFLOW_FINDINGS_COPY['investigation:v1']
  }

  return WORKFLOW_FINDINGS_COPY[workflowVersion]
}

export function triageCopyForGroup(
  group: FindingGroup | undefined,
  workflowVersion: WorkflowVersion,
): FindingTriageCopy {
  if (group && isInvestigationGroup(group)) {
    return INVESTIGATION_TRIAGE_COPY
  }

  return workflowVersion === 'investigation:v1' ? INVESTIGATION_TRIAGE_COPY : DELIVERY_TRIAGE_COPY
}

export function triageOptions(
  copy: FindingTriageCopy,
  hasLinkedItem: boolean,
  currentState?: FindingTriageState,
): { value: FindingTriageState; label: string; description: string }[] {
  const options: { value: FindingTriageState; label: string; description: string }[] = [
    { value: 'fix_now', label: copy.fixNowLabel, description: copy.fixNowDescription },
    { value: 'backlog', label: 'Backlog', description: copy.backlogDescription },
    { value: 'duplicate', label: 'Duplicate', description: 'Already tracked elsewhere' },
    { value: 'wont_fix', label: "Won't fix", description: 'Acceptable risk, note required' },
    { value: 'dismissed_invalid', label: 'Dismiss', description: 'False positive or invalid' },
    { value: 'needs_investigation', label: 'Investigate', description: 'Needs human analysis' },
  ]

  if (copy.mode === 'investigation' && hasLinkedItem) {
    return options.filter(
      (option) => (option.value !== 'fix_now' && option.value !== 'backlog') || option.value === currentState,
    )
  }

  return options
}

export function triageStateLabel(
  state: FindingTriageState,
  copy: FindingTriageCopy,
  linkedItemSummary?: LinkedFindingItemSummary,
): string {
  switch (state) {
    case 'untriaged':
      return 'Untriaged'
    case 'fix_now':
      return copy.fixNowLabel
    case 'wont_fix':
      return "Won't fix"
    case 'backlog':
      return copy.mode === 'investigation' && (linkedItemSummary?.job_count ?? 0) > 0 ? 'Fixing' : 'Backlog'
    case 'duplicate':
      return 'Duplicate'
    case 'dismissed_invalid':
      return 'Dismissed'
    case 'needs_investigation':
      return 'Investigating'
  }
}
