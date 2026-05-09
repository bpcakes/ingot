import { useQuery, useQueryClient } from '@tanstack/react-query'
import { agentsQuery, projectConfigQuery, projectsQuery, queryKeys } from '../api/queries'
import { CodeBlock } from '../components/CodeBlock'
import { AgentRoutingCard } from '../components/config/AgentRoutingCard'
import { AgentsTable } from '../components/config/AgentsTable'
import { AutoTriagePolicyCard } from '../components/config/AutoTriagePolicyCard'
import { ExecutionModeCard } from '../components/config/ExecutionModeCard'
import { RegisterAgentDialog } from '../components/config/RegisterAgentDialog'
import { PageHeader } from '../components/PageHeader'
import { PageQueryError } from '../components/PageQueryError'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '../components/ui/card'
import { Skeleton } from '../components/ui/skeleton'
import { useRequiredProjectId } from '../hooks/useRequiredRouteParam'

export default function ConfigPage(): React.JSX.Element {
  const projectId = useRequiredProjectId()
  const queryClient = useQueryClient()
  const {
    data: config,
    error: configError,
    isError: isConfigError,
    isFetching: isConfigFetching,
    isLoading: isConfigLoading,
    refetch: refetchConfig,
  } = useQuery(projectConfigQuery(projectId))
  const {
    data: agents,
    error: agentsError,
    isError: isAgentsError,
    isFetching: isAgentsFetching,
    isLoading: isAgentsLoading,
    refetch: refetchAgents,
  } = useQuery(agentsQuery())
  const { data: projects } = useQuery(projectsQuery())
  const project = projects?.find((p) => p.id === projectId)

  function refreshAgents(): void {
    queryClient.invalidateQueries({ queryKey: queryKeys.agents() })
  }

  if (isConfigError || isAgentsError) {
    return (
      <PageQueryError
        title="Config failed to load"
        error={configError ?? agentsError}
        onRetry={() => Promise.all([refetchConfig(), refetchAgents()])}
        isRetrying={isConfigFetching || isAgentsFetching}
      />
    )
  }

  return (
    <div className="space-y-8">
      <PageHeader
        title="Config"
        description="Set project defaults and register the agents that can execute queued work."
        action={<RegisterAgentDialog agents={agents} />}
      />

      <ExecutionModeCard projectId={projectId} executionMode={project?.execution_mode} />

      <AgentRoutingCard projectId={projectId} routing={project?.agent_routing ?? null} agents={agents} />

      <AutoTriagePolicyCard projectId={projectId} policy={project?.auto_triage_policy ?? null} />

      <Card>
        <CardHeader>
          <CardTitle>Project Defaults</CardTitle>
          <CardDescription>The resolved project configuration currently known to the daemon.</CardDescription>
        </CardHeader>
        <CardContent>
          {isConfigLoading ? (
            <div className="space-y-2">
              <Skeleton className="h-4 w-40" />
              <Skeleton className="h-40 w-full" />
            </div>
          ) : (
            <CodeBlock
              value={JSON.stringify(config ?? {}, null, 2)}
              copyLabel="Copy project defaults"
              maxHeightClassName="max-h-72"
            />
          )}
        </CardContent>
      </Card>

      <AgentsTable agents={agents} isLoading={isAgentsLoading} onReprobeSuccess={refreshAgents} />
    </div>
  )
}
