import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { useForm } from 'react-hook-form'
import { toast } from 'sonner'
import { createAgent } from '../../api/client'
import { queryKeys } from '../../api/queries'
import type { ComboboxOption } from '../../components/Combobox'
import { Combobox } from '../../components/Combobox'
import { showErrorToast } from '../../lib/toast'
import type { Agent, AgentProvider } from '../../types/domain'
import { Button } from '../ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle, DialogTrigger } from '../ui/dialog'
import { Form, FormControl, FormField, FormItem, FormLabel, FormMessage } from '../ui/form'
import { Input } from '../ui/input'

type AgentForm = {
  name: string
  provider: AgentProvider
  model: string
  cliPath: string
}

const INITIAL_AGENT_FORM: AgentForm = {
  name: 'Codex CLI',
  provider: 'openai',
  model: 'gpt-5-codex',
  cliPath: 'codex',
}

const PROVIDER_DEFAULTS: AgentProvider[] = ['openai', 'anthropic']
const PROVIDER_MODEL_DEFAULTS: Record<AgentProvider, string[]> = {
  openai: ['gpt-5-codex', 'gpt-5'],
  anthropic: [],
}

type RegisterAgentDialogProps = {
  agents: Agent[] | undefined
}

export function RegisterAgentDialog({ agents }: RegisterAgentDialogProps): React.JSX.Element {
  const queryClient = useQueryClient()
  const [dialogOpen, setDialogOpen] = useState(false)
  const form = useForm<AgentForm>({
    defaultValues: INITIAL_AGENT_FORM,
  })
  const selectedProvider = form.watch('provider')
  const selectedModel = form.watch('model')
  const providerOptions = buildProviderOptions(agents)
  const modelOptions = buildModelOptions(selectedProvider, selectedModel, agents)

  const createAgentMutation = useMutation({
    mutationFn: (values: AgentForm) =>
      createAgent({
        name: values.name,
        adapter_kind: 'codex',
        provider: values.provider,
        model: values.model,
        cli_path: values.cliPath,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.agents() })
      handleDialogOpenChange(false)
      toast.success('Agent registered.')
    },
    onError: (error) => {
      showErrorToast('Agent registration failed.', error)
    },
  })

  function handleDialogOpenChange(open: boolean): void {
    setDialogOpen(open)
    if (!open) {
      form.reset(INITIAL_AGENT_FORM)
      createAgentMutation.reset()
    }
  }

  return (
    <Dialog open={dialogOpen} onOpenChange={handleDialogOpenChange}>
      <DialogTrigger asChild>
        <Button type="button">Register Codex agent</Button>
      </DialogTrigger>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Register Agent</DialogTitle>
          <DialogDescription>
            Define the adapter, provider, model, and CLI path available for project execution.
          </DialogDescription>
        </DialogHeader>
        <Form {...form}>
          <form onSubmit={form.handleSubmit((values) => createAgentMutation.mutate(values))} className="grid gap-4">
            <FormField
              control={form.control}
              name="name"
              rules={{ required: 'Agent name is required.' }}
              render={({ field }) => (
                <FormItem>
                  <FormLabel>Agent name</FormLabel>
                  <FormControl>
                    <Input placeholder="Agent name" {...field} />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <FormField
              control={form.control}
              name="provider"
              rules={{ required: 'Provider is required.' }}
              render={({ field }) => (
                <FormItem>
                  <FormLabel>Provider</FormLabel>
                  <FormControl>
                    <Combobox
                      ariaLabel="Provider"
                      value={field.value}
                      onChange={(provider) => {
                        if (!isAgentProvider(provider)) {
                          return
                        }
                        const previousProvider = form.getValues('provider')
                        const currentModel = form.getValues('model')
                        field.onChange(provider)

                        const previousDefaults = PROVIDER_MODEL_DEFAULTS[previousProvider] ?? []
                        const nextDefaults = PROVIDER_MODEL_DEFAULTS[provider] ?? []
                        if (!currentModel || previousDefaults.includes(currentModel)) {
                          form.setValue('model', nextDefaults[0] ?? '', {
                            shouldDirty: true,
                            shouldValidate: true,
                          })
                        }
                      }}
                      options={providerOptions}
                      placeholder="Select provider"
                      searchPlaceholder="Filter providers..."
                      emptyText="No providers found."
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <FormField
              control={form.control}
              name="model"
              rules={{ required: 'Model is required.' }}
              render={({ field }) => (
                <FormItem>
                  <FormLabel>Model</FormLabel>
                  <FormControl>
                    <Combobox
                      ariaLabel="Model"
                      value={field.value}
                      onChange={field.onChange}
                      options={modelOptions}
                      placeholder="Select or type a model"
                      searchPlaceholder="Filter models..."
                      emptyText="No saved models for this provider."
                      allowCustom
                      customLabel={(query) => `Use "${query}"`}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <FormField
              control={form.control}
              name="cliPath"
              rules={{ required: 'CLI path is required.' }}
              render={({ field }) => (
                <FormItem>
                  <FormLabel>CLI path</FormLabel>
                  <FormControl>
                    <Input placeholder="CLI path" {...field} />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <div className="flex items-center gap-3">
              <Button type="submit" disabled={createAgentMutation.isPending}>
                {createAgentMutation.isPending ? 'Registering…' : 'Register Codex agent'}
              </Button>
              <Button type="button" variant="outline" onClick={() => handleDialogOpenChange(false)}>
                Cancel
              </Button>
            </div>
          </form>
        </Form>
      </DialogContent>
    </Dialog>
  )
}

function isAgentProvider(value: string): value is AgentProvider {
  return value === 'openai' || value === 'anthropic'
}

function toComboboxOptions(values: Iterable<string>): ComboboxOption[] {
  return Array.from(values, (value) => ({
    value,
    label: value,
  }))
}

function buildProviderOptions(agents: Agent[] | undefined): ComboboxOption[] {
  const knownProviders = new Set(PROVIDER_DEFAULTS)

  for (const agent of agents ?? []) {
    knownProviders.add(agent.provider)
  }

  return toComboboxOptions(knownProviders)
}

function buildModelOptions(
  selectedProvider: AgentProvider,
  selectedModel: string,
  agents: Agent[] | undefined,
): ComboboxOption[] {
  const knownModels = new Set(PROVIDER_MODEL_DEFAULTS[selectedProvider] ?? [])

  for (const agent of agents ?? []) {
    if (agent.provider === selectedProvider) {
      knownModels.add(agent.model)
    }
  }

  if (selectedModel) {
    knownModels.add(selectedModel)
  }

  return toComboboxOptions(knownModels)
}
