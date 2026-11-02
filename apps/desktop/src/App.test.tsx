import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { makeQueryClient } from './queryClient'
import type { Project, SessionInfo, SessionMeta } from './services/types'
import { useUi } from './stores/ui'
import App from './App'

type Handler = (e: { payload: unknown }) => void
const { handlers, listen, service, projects, agents, log } = vi.hoisted(() => {
  const handlers = new Map<string, Set<Handler>>()
  const listen = vi.fn(async (name: string, cb: Handler) => {
    if (!handlers.has(name)) handlers.set(name, new Set())
    handlers.get(name)!.add(cb)
    return () => void handlers.get(name)!.delete(cb)
  })
  return {
    handlers,
    listen,
    log: [] as string[],
    service: { list: vi.fn(), status: vi.fn(), create: vi.fn(), stop: vi.fn(), meta: vi.fn() },
    projects: { list: vi.fn(), add: vi.fn(), remove: vi.fn() },
    agents: { list: vi.fn() },
  }
})
vi.mock('@tauri-apps/api/event', () => ({ listen }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
vi.mock('./services/session', () => ({ sessionService: service }))
vi.mock('./services/projects', () => ({ projectService: projects }))
vi.mock('./services/agents', () => ({ agentService: agents }))
// jsdom has no layout, so the xterm-based controller is replaced; its own tests cover it.
vi.mock('./features/terminal/TerminalController', () => ({
  TerminalController: class {
    constructor(private id: string) {
      log.push(`new:${id}`)
    }
    async mount() {}
    dispose() {
      log.push(`dispose:${this.id}`)
    }
  },
}))

const session = (id: string, over: Partial<SessionInfo> = {}): SessionInfo => ({
  id,
  title: `title-${id}`,
  argv: ['sh'],
  cwd: '/tmp',
  state: 'running',
  exit_code: null,
  error: null,
  started_at: Date.now(),
  ended_at: null,
  ...over,
})
const meta = (sessionId: string, over: Partial<SessionMeta> = {}): SessionMeta => ({
  session_id: sessionId,
  project_id: 'p1',
  agent: 'shell',
  argv: ['/bin/zsh'],
  cwd: '/tmp',
  title: '',
  created_at: 1,
  ...over,
})
const project = (id: string, over: Partial<Project> = {}): Project => ({
  id,
  name: id,
  root: `/${id}`,
  created_at: 1,
  missing: false,
  ...over,
})
const status = (connected: boolean) => ({ connected, default_shell: '/bin/zsh', home: '/h' })
const listeners = () => [...handlers.values()].reduce((n, set) => n + set.size, 0)
const emit = (name: string, payload: unknown) => act(() => void handlers.get(name)?.forEach((cb) => cb({ payload })))

let qc: QueryClient
const renderApp = () =>
  render(
    <QueryClientProvider client={qc}>
      <App />
    </QueryClientProvider>,
  )

const sessionButtons = () => screen.queryAllByRole('button', { name: /title-/ })

beforeEach(async () => {
  qc = makeQueryClient()
  handlers.clear()
  log.length = 0
  Object.values(service).forEach((f) => f.mockReset())
  Object.values(projects).forEach((f) => f.mockReset())
  agents.list.mockReset().mockResolvedValue([])
  service.list.mockResolvedValue([session('a'), session('b')])
  service.meta.mockResolvedValue([])
  service.status.mockResolvedValue(status(true))
  projects.list.mockResolvedValue([])
  useUi.setState({ selectedId: null })
})

describe('App', () => {
  it('renders a row per session and mounts a terminal for the one selected', async () => {
    renderApp()
    expect((await screen.findAllByRole('button', { name: /title-/ })).length).toBe(2)
    expect(screen.getByText(/select or create a session/i)).toBeTruthy()
    expect(log).toEqual([])

    fireEvent.click(screen.getByRole('button', { name: /title-b/ }))
    expect(log).toEqual(['new:b'])
    expect(screen.getByRole('button', { name: /title-b/ }).getAttribute('aria-current')).toBe('true')
    expect(screen.getByRole('button', { name: /title-a/ }).getAttribute('aria-current')).toBe('false')
    expect(screen.getByRole('button', { name: 'Stop' })).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: /title-a/ }))
    expect(log).toEqual(['new:b', 'dispose:b', 'new:a'])
  })

  it('shows the banner and disables controls while disconnected, and recovers', async () => {
    renderApp()
    await vi.waitFor(() => expect(listeners()).toBe(3))
    fireEvent.click(await screen.findByRole('button', { name: /title-a/ }))
    const stop = screen.getByRole('button', { name: 'Stop' }) as HTMLButtonElement
    expect(stop.disabled).toBe(false)
    expect(screen.queryByRole('alert')).toBeNull()

    emit('daemon://status', { connected: false })
    expect((await screen.findByRole('alert')).textContent).toMatch(/unreachable/i)
    expect(screen.getByRole('status').getAttribute('aria-label')).toBe('Daemon disconnected')
    expect(stop.disabled).toBe(true)

    emit('daemon://status', { connected: true }) // the bridge reconnected: both keys refetch
    await waitFor(() => expect(screen.queryByRole('alert')).toBeNull())
    expect(screen.getByRole('status').getAttribute('aria-label')).toBe('Daemon connected')
    expect(stop.disabled).toBe(false)
  })

  it('shows no sessions after reconnecting to a daemon with an empty session list', async () => {
    renderApp()
    await waitFor(() => expect(sessionButtons()).toHaveLength(2))
    await waitFor(() => expect(listeners()).toBe(3))

    service.list.mockResolvedValue([])
    emit('daemon://status', { connected: false })
    await screen.findByRole('alert')
    emit('daemon://status', { connected: true })

    await waitFor(() => expect(sessionButtons()).toHaveLength(0))
    expect(screen.getByText(/no projects yet/i)).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Stop' })).toBeNull()
  })

  it('applies pushed session events to the list', async () => {
    renderApp()
    await vi.waitFor(() => expect(listeners()).toBe(3))
    await screen.findAllByRole('button', { name: /title-/ })
    emit('session://updated', session('c'))
    expect(await screen.findByRole('button', { name: /title-c/ })).toBeTruthy()
  })

  it('stops the event sync on unmount', async () => {
    const { unmount } = renderApp()
    await vi.waitFor(() => expect(listeners()).toBe(3))
    unmount()
    await vi.waitFor(() => expect(listeners()).toBe(0))
  })

  it('nests sessions under their project and shows project, agent, and title above the terminal', async () => {
    const p1 = project('p1', { name: 'one', root: '/w/one' })
    projects.list.mockResolvedValue([p1])
    agents.list.mockResolvedValue([{ id: 'claude', label: 'Claude Code', available: true, path: '/bin/claude' }])
    service.meta.mockResolvedValue([meta('a', { project_id: 'p1', agent: 'claude' }), meta('b', { project_id: null })])
    renderApp()
    await waitFor(() => expect(sessionButtons()).toHaveLength(2))

    const node = screen.getByRole('button', { name: /one.*\/w\/one/ }).closest('li')!
    expect(within(node).getByRole('button', { name: /title-a/ })).toBeTruthy()
    expect(within(node).queryByRole('button', { name: /title-b/ })).toBeNull()
    expect(within(screen.getByText('Unassigned').closest('li')!).getByRole('button', { name: /title-b/ })).toBeTruthy()

    fireEvent.click(within(node).getByRole('button', { name: /title-a/ }))
    expect(log).toEqual(['new:a'])
    await waitFor(() => expect(screen.getByText(/one ›/)).toBeTruthy())
    expect(screen.getByText(/Claude Code ·/)).toBeTruthy()
  })

  it('starting an agent from a project selects the new session and mounts its terminal', async () => {
    const p1 = project('p1', { name: 'one', root: '/w/one' })
    projects.list.mockResolvedValue([p1])
    service.list.mockResolvedValue([])
    agents.list.mockResolvedValue([{ id: 'shell', label: 'Shell', available: true, path: '/bin/zsh' }])
    service.create.mockResolvedValue(session('new'))
    service.meta.mockResolvedValueOnce([]).mockResolvedValue([meta('new', { project_id: 'p1' })])
    renderApp()

    fireEvent.keyDown(await screen.findByRole('button', { name: 'New session in one' }), { key: 'ArrowDown' })
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Shell' }))

    await waitFor(() => expect(log).toEqual(['new:new']))
    expect(service.create).toHaveBeenCalledWith({ agent: 'shell', projectId: 'p1', cols: 80, rows: 24 }, expect.anything())
    expect(useUi.getState().selectedId).toBe('new')
  })

  it('removing a project never stops its sessions', async () => {
    const p1 = project('p1', { name: 'one', root: '/w/one' })
    projects.list.mockResolvedValueOnce([p1]).mockResolvedValue([])
    service.meta.mockResolvedValue([meta('a', { project_id: 'p1' })])
    projects.remove.mockResolvedValue(undefined)
    renderApp()
    await screen.findByRole('button', { name: 'Remove one' })

    fireEvent.click(screen.getByRole('button', { name: 'Remove one' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Remove' }))
    await waitFor(() => expect(screen.getByText('Unassigned')).toBeTruthy()) // a survives, unassigned
    expect(sessionButtons()).toHaveLength(2)
    expect(service.stop).not.toHaveBeenCalled()
  })
})
