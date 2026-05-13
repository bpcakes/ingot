import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router'
import { Toaster } from '../components/ui/sonner'
import ProjectsPage from '../pages/ProjectsPage'

function jsonResponse(body: unknown) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: {
      'Content-Type': 'application/json',
    },
  })
}

function renderPage() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
      },
    },
  })

  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={['/']}>
        <Routes>
          <Route path="/" element={<ProjectsPage />} />
        </Routes>
        <Toaster />
      </MemoryRouter>
    </QueryClientProvider>,
  )
}

describe('ProjectsPage', () => {
  afterEach(() => {
    vi.restoreAllMocks()
    if (typeof window !== 'undefined') {
      delete window.ingotDesktop
    }
  })

  it('opens the registration dialog and renders the linked project list', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation((input) => {
      const url = String(input)
      if (url.endsWith('/api/projects')) {
        return Promise.resolve(
          jsonResponse([
            {
              id: 'prj_1',
              name: 'Ingot',
              path: '/tmp/ingot',
              default_branch: 'main',
              color: '#1f2937',
            },
          ]),
        )
      }
      throw new Error(`Unexpected fetch: ${url}`)
    })

    renderPage()

    expect(await screen.findByRole('button', { name: 'Register project' })).toBeInTheDocument()
    expect(await screen.findByRole('link', { name: /Ingot/i })).toHaveAttribute('href', '/projects/prj_1')
    expect(screen.getByText('main')).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Register project' }))

    expect(await screen.findByRole('dialog')).toBeInTheDocument()
    expect(screen.getByText('Register Project')).toBeInTheDocument()
    expect(screen.getByLabelText('Repository path')).toBeInTheDocument()
  })

  it('shows a required-field message when the repository path is missing', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation((input) => {
      const url = String(input)
      if (url.endsWith('/api/projects')) {
        return Promise.resolve(jsonResponse([]))
      }
      throw new Error(`Unexpected fetch: ${url}`)
    })

    renderPage()

    fireEvent.click(await screen.findByRole('button', { name: 'Register project' }))
    const dialog = await screen.findByRole('dialog')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Register project' }))

    expect(await screen.findByText('Repository path is required.')).toBeInTheDocument()
  })

  it('fills the repository path from the desktop directory picker', async () => {
    window.ingotDesktop = {
      pickProjectDirectory: vi.fn().mockResolvedValue('/Users/test/project'),
    }
    vi.spyOn(globalThis, 'fetch').mockImplementation((input) => {
      const url = String(input)
      if (url.endsWith('/api/projects')) {
        return Promise.resolve(jsonResponse([]))
      }
      throw new Error(`Unexpected fetch: ${url}`)
    })

    renderPage()

    fireEvent.click(await screen.findByRole('button', { name: 'Register project' }))
    const dialog = await screen.findByRole('dialog')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Browse' }))

    expect(window.ingotDesktop.pickProjectDirectory).toHaveBeenCalledTimes(1)
    expect(await within(dialog).findByDisplayValue('/Users/test/project')).toBeInTheDocument()
  })

  it('leaves the repository path unchanged when the desktop directory picker is canceled', async () => {
    let resolvePath: (path: string | null) => void = () => {}
    window.ingotDesktop = {
      pickProjectDirectory: vi.fn(
        () =>
          new Promise<string | null>((resolve) => {
            resolvePath = resolve
          }),
      ),
    }
    vi.spyOn(globalThis, 'fetch').mockImplementation((input) => {
      const url = String(input)
      if (url.endsWith('/api/projects')) {
        return Promise.resolve(jsonResponse([]))
      }
      throw new Error(`Unexpected fetch: ${url}`)
    })

    renderPage()

    fireEvent.click(await screen.findByRole('button', { name: 'Register project' }))
    const dialog = await screen.findByRole('dialog')
    const pathInput = within(dialog).getByLabelText('Repository path')
    fireEvent.change(pathInput, { target: { value: '/Users/test/existing' } })
    const browseButton = within(dialog).getByRole('button', { name: 'Browse' })
    fireEvent.click(browseButton)

    expect(window.ingotDesktop.pickProjectDirectory).toHaveBeenCalledTimes(1)
    resolvePath(null)
    await waitFor(() => expect(browseButton).toBeEnabled())
    expect(within(dialog).getByDisplayValue('/Users/test/existing')).toBeInTheDocument()
  })

  it('disables the browse button while the desktop directory picker is open', async () => {
    let resolvePath: (path: string) => void = () => {}
    window.ingotDesktop = {
      pickProjectDirectory: vi.fn(
        () =>
          new Promise<string>((resolve) => {
            resolvePath = resolve
          }),
      ),
    }
    vi.spyOn(globalThis, 'fetch').mockImplementation((input) => {
      const url = String(input)
      if (url.endsWith('/api/projects')) {
        return Promise.resolve(jsonResponse([]))
      }
      throw new Error(`Unexpected fetch: ${url}`)
    })

    renderPage()

    fireEvent.click(await screen.findByRole('button', { name: 'Register project' }))
    const dialog = await screen.findByRole('dialog')
    const browseButton = within(dialog).getByRole('button', { name: 'Browse' })
    fireEvent.click(browseButton)

    expect(browseButton).toBeDisabled()
    resolvePath('/Users/test/project')
    expect(await within(dialog).findByDisplayValue('/Users/test/project')).toBeInTheDocument()
    expect(browseButton).toBeEnabled()
  })

  it('renders a toast when the desktop directory picker fails', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {})
    const pickerError = new Error('untrusted renderer')
    window.ingotDesktop = {
      pickProjectDirectory: vi.fn().mockRejectedValue(pickerError),
    }
    vi.spyOn(globalThis, 'fetch').mockImplementation((input) => {
      const url = String(input)
      if (url.endsWith('/api/projects')) {
        return Promise.resolve(jsonResponse([]))
      }
      throw new Error(`Unexpected fetch: ${url}`)
    })

    renderPage()

    fireEvent.click(await screen.findByRole('button', { name: 'Register project' }))
    const dialog = await screen.findByRole('dialog')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Browse' }))

    expect(window.ingotDesktop.pickProjectDirectory).toHaveBeenCalledTimes(1)
    expect(await screen.findByText('Path picker failed.')).toBeInTheDocument()
    expect(consoleError).toHaveBeenCalledWith(pickerError)
    expect(screen.getByText('Path picker unavailable.')).toBeInTheDocument()
    expect(screen.queryByText('untrusted renderer')).not.toBeInTheDocument()
  })

  it('renders a destructive alert when the projects query fails', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation((input) => {
      const url = String(input)
      if (url.endsWith('/api/projects')) {
        return Promise.reject(new Error('network down'))
      }
      throw new Error(`Unexpected fetch: ${url}`)
    })

    renderPage()

    expect(await screen.findByText('Projects failed to load')).toBeInTheDocument()
    expect(screen.getByText('Error: network down')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument()
  })
})
