import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { updateProject } from '../../api/client'
import { queryKeys } from '../../api/queries'
import { showErrorToast } from '../../lib/toast'
import type { Agent, AgentRouting } from '../../types/domain'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '../ui/card'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../ui/select'

const ROUTING_PHASES = [
  { key: 'author' as const, label: 'Author' },
  { key: 'review' as const, label: 'Review' },
  { key: 'investigate' as const, label: 'Investigate' },
]

const AUTO_VALUE = '__auto__'

type AgentRoutingCardProps = {
  projectId: string
  routing: AgentRouting | null
  agents: Agent[] | undefined
}

export function AgentRoutingCard({ projectId, routing, agents }: AgentRoutingCardProps): React.JSX.Element {
  const queryClient = useQueryClient()
  const [draftRouting, setDraftRouting] = useState<AgentRouting | null>(routing)
  const routingMutation = useMutation({
    mutationFn: (newRouting: AgentRouting) => updateProject(projectId, { agent_routing: newRouting }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.projects() })
      toast.success('Agent routing updated.')
    },
    onError: (error) => {
      setDraftRouting(routing)
      showErrorToast('Failed to update agent routing', error)
    },
  })

  useEffect(() => {
    setDraftRouting(routing)
  }, [routing])

  const current: AgentRouting = draftRouting ?? { author: null, review: null, investigate: null }

  function handleChange(phase: keyof AgentRouting, value: string): void {
    const slug = value === AUTO_VALUE ? null : value
    const nextRouting = { ...current, [phase]: slug }
    setDraftRouting(nextRouting)
    routingMutation.mutate(nextRouting)
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>Agent Routing</CardTitle>
        <CardDescription>
          Choose which agent handles each workflow phase. Default (auto) picks the first available.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <div className="grid gap-4 sm:grid-cols-3">
          {ROUTING_PHASES.map(({ key, label }) => (
            <div key={key} className="space-y-1.5">
              <span className="text-sm font-medium">{label}</span>
              <Select
                value={current[key] ?? AUTO_VALUE}
                onValueChange={(v) => handleChange(key, v)}
                disabled={routingMutation.isPending}
              >
                <SelectTrigger aria-label={`${label} agent`}>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={AUTO_VALUE}>Default (auto)</SelectItem>
                  {agents?.map((agent) => (
                    <SelectItem key={agent.id} value={agent.slug}>
                      {agent.name} ({agent.slug})
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  )
}
