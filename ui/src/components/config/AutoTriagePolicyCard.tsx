import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { updateProject } from '../../api/client'
import { queryKeys } from '../../api/queries'
import { showErrorToast } from '../../lib/toast'
import type { AutoTriageDecision, AutoTriagePolicy } from '../../types/domain'
import { Button } from '../ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '../ui/card'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../ui/select'

const DEFAULT_AUTO_TRIAGE_POLICY: AutoTriagePolicy = {
  critical: 'fix_now',
  high: 'fix_now',
  medium: 'fix_now',
  low: 'backlog',
}

const SEVERITY_ROWS: { key: keyof AutoTriagePolicy; label: string }[] = [
  { key: 'critical', label: 'Critical' },
  { key: 'high', label: 'High' },
  { key: 'medium', label: 'Medium' },
  { key: 'low', label: 'Low' },
]

const TRIAGE_DECISION_OPTIONS: { value: AutoTriageDecision; label: string }[] = [
  { value: 'fix_now', label: 'Fix now' },
  { value: 'backlog', label: 'Backlog' },
  { value: 'skip', label: 'Skip (manual)' },
]

type AutoTriagePolicyCardProps = {
  projectId: string
  policy: AutoTriagePolicy | null
}

export function AutoTriagePolicyCard({ projectId, policy }: AutoTriagePolicyCardProps): React.JSX.Element {
  const queryClient = useQueryClient()
  const [draftPolicy, setDraftPolicy] = useState<AutoTriagePolicy | null>(policy)
  const mutation = useMutation({
    mutationFn: (newPolicy: AutoTriagePolicy | null) => updateProject(projectId, { auto_triage_policy: newPolicy }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.projects() })
      toast.success('Auto-triage policy updated.')
    },
    onError: (error) => {
      setDraftPolicy(policy)
      showErrorToast('Failed to update auto-triage policy', error)
    },
  })

  useEffect(() => {
    setDraftPolicy(policy)
  }, [policy])

  const enabled = draftPolicy !== null
  const current = draftPolicy ?? DEFAULT_AUTO_TRIAGE_POLICY

  function handleToggle(): void {
    const nextPolicy = enabled ? null : DEFAULT_AUTO_TRIAGE_POLICY
    setDraftPolicy(nextPolicy)
    mutation.mutate(nextPolicy)
  }

  function handleChange(severity: keyof AutoTriagePolicy, value: string): void {
    const nextPolicy = { ...current, [severity]: value as AutoTriageDecision }
    setDraftPolicy(nextPolicy)
    mutation.mutate(nextPolicy)
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>Auto-Triage Policy</CardTitle>
        <CardDescription>
          When enabled in autopilot mode, findings are automatically triaged by severity instead of blocking for human
          review.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex items-center gap-3">
          <Button
            variant={enabled ? 'default' : 'outline'}
            size="sm"
            onClick={handleToggle}
            disabled={mutation.isPending}
          >
            {enabled ? 'Enabled' : 'Disabled'}
          </Button>
        </div>
        {enabled && (
          <div className="grid gap-4 sm:grid-cols-4">
            {SEVERITY_ROWS.map(({ key, label }) => (
              <div key={key} className="space-y-1.5">
                <span className="text-sm font-medium">{label}</span>
                <Select value={current[key]} onValueChange={(v) => handleChange(key, v)} disabled={mutation.isPending}>
                  <SelectTrigger aria-label={`${label} triage decision`}>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {TRIAGE_DECISION_OPTIONS.map((opt) => (
                      <SelectItem key={opt.value} value={opt.value}>
                        {opt.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  )
}
