import { useMutation } from '@tanstack/react-query'
import { toast } from 'sonner'
import { reprobeAgent } from '../../api/client'
import { showErrorToast } from '../../lib/toast'
import type { Agent } from '../../types/domain'
import { DataTable } from '../DataTable'
import { EmptyState } from '../EmptyState'
import { StatusBadge } from '../StatusBadge'
import { Button } from '../ui/button'
import { Skeleton } from '../ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '../ui/table'

type AgentsTableProps = {
  agents: Agent[] | undefined
  isLoading: boolean
  onReprobeSuccess: () => void
}

export function AgentsTable({ agents, isLoading, onReprobeSuccess }: AgentsTableProps): React.JSX.Element {
  return (
    <DataTable
      title="Agents"
      description="Reprobe health and confirm which execution endpoints are currently available."
    >
      {isLoading ? (
        <div className="space-y-4 px-4 py-4">
          <div className="grid grid-cols-6 gap-3">
            {['name', 'adapter', 'model', 'status', 'health', 'actions'].map((key) => (
              <Skeleton key={key} className="h-4 w-16" />
            ))}
          </div>
          {['row-1', 'row-2', 'row-3', 'row-4'].map((rowKey) => (
            <div key={rowKey} className="grid grid-cols-6 gap-3">
              {['name', 'adapter', 'model', 'status', 'health', 'actions'].map((columnKey) => (
                <Skeleton key={`${rowKey}-${columnKey}`} className="h-5 w-full" />
              ))}
            </div>
          ))}
        </div>
      ) : agents && agents.length > 0 ? (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Name</TableHead>
              <TableHead>Adapter</TableHead>
              <TableHead>Model</TableHead>
              <TableHead>Status</TableHead>
              <TableHead>Health</TableHead>
              <TableHead>Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {agents.map((agent) => (
              <AgentRow key={agent.id} agent={agent} onSuccess={onReprobeSuccess} />
            ))}
          </TableBody>
        </Table>
      ) : (
        <EmptyState variant="inline" description="No agents configured." />
      )}
    </DataTable>
  )
}

type AgentRowProps = {
  agent: Agent
  onSuccess: () => void
}

function AgentRow({ agent, onSuccess }: AgentRowProps): React.JSX.Element {
  const reprobeMutation = useMutation({
    mutationFn: () => reprobeAgent(agent.id),
    onSuccess: () => {
      onSuccess()
      toast.success(`Reprobe complete for ${agent.name}.`)
    },
    onError: (error) => {
      showErrorToast('Reprobe failed.', error)
    },
  })

  return (
    <TableRow>
      <TableCell>{agent.name}</TableCell>
      <TableCell>{agent.adapter_kind}</TableCell>
      <TableCell>{agent.model}</TableCell>
      <TableCell>
        <StatusBadge status={agent.status} />
      </TableCell>
      <TableCell className="whitespace-normal">{agent.health_check ?? '—'}</TableCell>
      <TableCell className="whitespace-normal">
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => reprobeMutation.mutate()}
          disabled={reprobeMutation.isPending}
        >
          {reprobeMutation.isPending ? 'Reprobing…' : 'Reprobe'}
        </Button>
      </TableCell>
    </TableRow>
  )
}
