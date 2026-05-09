import {
  groupFindingsByJob,
  INVESTIGATION_TRIAGE_COPY,
  triageCopyForGroup,
  triageOptions,
  triageStateLabel,
} from '../components/item-detail/findingsPresentation'
import type { Finding, Job, LinkedFindingItemSummary } from '../types/domain'

function finding(params: {
  id: string
  jobId: string
  stepId: string
  createdAt: string
  investigation?: Finding['investigation']
}): Finding {
  return {
    id: params.id,
    source_job_id: params.jobId,
    source_step_id: params.stepId,
    created_at: params.createdAt,
    investigation: params.investigation ?? null,
  } as Finding
}

function job(params: { id: string; endedAt: string; phaseKind?: Job['phase_kind'] }): Job {
  return {
    id: params.id,
    ended_at: params.endedAt,
    phase_kind: params.phaseKind ?? 'review',
  } as Job
}

describe('findings presentation helpers', () => {
  it('groups findings by job and marks the most recent group', () => {
    const olderJob = job({ id: 'job_1', endedAt: '2026-03-11T00:00:00Z' })
    const newerJob = job({ id: 'job_2', endedAt: '2026-03-12T00:00:00Z' })

    const groups = groupFindingsByJob(
      [
        finding({
          id: 'fnd_1',
          jobId: olderJob.id,
          stepId: 'review_candidate_initial',
          createdAt: '2026-03-11T00:00:00Z',
        }),
        finding({
          id: 'fnd_2',
          jobId: newerJob.id,
          stepId: 'investigate_item',
          createdAt: '2026-03-12T00:00:00Z',
        }),
      ],
      [olderJob, newerJob],
    )

    expect(groups.map((group) => group.jobId)).toEqual(['job_2', 'job_1'])
    expect(groups[0]?.isLatest).toBe(true)
    expect(groups[1]?.isLatest).toBe(false)
  })

  it('uses investigation triage copy for investigation-origin findings on delivery items', () => {
    const groups = groupFindingsByJob(
      [
        finding({
          id: 'fnd_1',
          jobId: 'job_1',
          stepId: 'investigate_item',
          createdAt: '2026-03-12T00:00:00Z',
        }),
      ],
      [job({ id: 'job_1', endedAt: '2026-03-12T00:00:00Z' })],
    )

    expect(triageCopyForGroup(groups[0], 'delivery:v1').mode).toBe('investigation')
  })

  it('hides duplicate launch choices for already-linked investigation findings', () => {
    const options = triageOptions(INVESTIGATION_TRIAGE_COPY, true)

    expect(options.map((option) => option.value)).not.toContain('fix_now')
    expect(options.map((option) => option.value)).not.toContain('backlog')
  })

  it('labels launched investigation backlog items as fixing', () => {
    const linkedItem = { job_count: 1 } as LinkedFindingItemSummary

    expect(triageStateLabel('backlog', INVESTIGATION_TRIAGE_COPY, linkedItem)).toBe('Fixing')
  })
})
