import { useMutation, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { updateProject } from '../../api/client'
import { queryKeys } from '../../api/queries'
import { showErrorToast } from '../../lib/toast'
import { Button } from '../ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '../ui/card'

type ExecutionMode = 'manual' | 'autopilot'

type ExecutionModeCardProps = {
  projectId: string
  executionMode: ExecutionMode | undefined
}

export function ExecutionModeCard({ projectId, executionMode }: ExecutionModeCardProps): React.JSX.Element {
  const queryClient = useQueryClient()
  const executionModeMutation = useMutation({
    mutationFn: (mode: ExecutionMode) => updateProject(projectId, { execution_mode: mode }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.projects() })
      queryClient.invalidateQueries({ queryKey: queryKeys.items(projectId) })
      toast.success('Execution mode updated.')
    },
    onError: (error) => showErrorToast('Failed to update execution mode', error),
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>Execution Mode</CardTitle>
        <CardDescription>
          In autopilot mode, the daemon automatically dispatches every safe workflow step until it hits a human gate
          (approval, escalation, findings triage, or conflict).
        </CardDescription>
      </CardHeader>
      <CardContent>
        <div className="flex items-center gap-3">
          <Button
            variant={executionMode === 'manual' ? 'default' : 'outline'}
            size="sm"
            onClick={() => executionModeMutation.mutate('manual')}
            disabled={executionModeMutation.isPending || executionMode === 'manual'}
          >
            Manual
          </Button>
          <Button
            variant={executionMode === 'autopilot' ? 'default' : 'outline'}
            size="sm"
            onClick={() => executionModeMutation.mutate('autopilot')}
            disabled={executionModeMutation.isPending || executionMode === 'autopilot'}
          >
            Autopilot
          </Button>
        </div>
      </CardContent>
    </Card>
  )
}
