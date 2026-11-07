import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { open } from '@tauri-apps/plugin-dialog'
import { makeQueryClient } from './queryClient'
import type { Project, SessionInfo, SessionMeta } from './services/types'
import { useUi } from './stores/ui'
import App from './App'

type Handler = (e: { payload: unknown }) => void
const { handlers, listen, service, projects, agents, worktrees, gitview, settings, usageService, activityService, log } =
  vi.hoisted(() => {
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
      service: { list: vi.fn(), status: vi.fn(), create: vi.fn(), stop: vi.fn(), delete: vi.fn(), meta: vi.fn() },
      projects: { list: vi.fn(), add: vi.fn(), remove: vi.fn() },
      agents: { list: vi.fn() },
      worktrees: { list: vi.fn(), branches: vi.fn(), create: vi.fn(), envCandidates: vi.fn(), envCopy: vi.fn(), remove: vi.fn() },
      gitview: { changes: vi.fn(), diff: vi.fn(), history: vi.fn(), branches: vi.fn(), tree: vi.fn(), file: vi.fn(), stashes: vi.fn(), stashDiff: vi.fn() },
      settings: { get: vi.fn(), set: vi.fn() },
      usageService: { get: vi.fn() },
      activityService: { get: vi.fn() },
    }
  })
vi.mock('@tauri-apps/api/event', () => ({ listen }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
vi.mock('./services/session', () => ({ sessionService: service }))
vi.mock('./services/projects', () => ({ projectService: projects }))
vi.mock('./services/agents', () => ({ agentService: agents }))
vi.mock('./services/worktrees', () => ({ worktreeService: worktrees }))
vi.mock('./services/git-view', () => ({ gitViewService: gitview }))
vi.mock('./services/settings', () => ({ settingsService: settings }))
vi.mock('./services/usage', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./services/usage')>()),
  usageService,
}))
vi.mock('./services/activity', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./services/activity')>()),
  activityService,
}))
// jsdom has no layout, so the xterm-based controller is replaced; its own tests cover it.
vi.mock('./features/terminal/TerminalController', () => ({
  TerminalController: class {
    constructor(private id: string) {
      log.push(`new:${id}`)
    }
    async mount() {}
    setVisible() {}
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
  Object.values(worktrees).forEach((f) => f.mockReset())
  activityService.get.mockReset().mockResolvedValue({})
  usageService.get.mockReset().mockResolvedValue({
    claude: { five_hour: null, seven_day: null, sessions: {}, updated_at: null },
    codex: { primary: null, secondary: null, updated_at: null },
  })
  worktrees.list.mockResolvedValue({ repo: false, worktrees: [] })
  worktrees.branches.mockResolvedValue({ branches: [], bases: [] })
  worktrees.envCandidates.mockResolvedValue([])
  worktrees.envCopy.mockResolvedValue({ copied: [], skipped: [], failed: [] })
  worktrees.remove.mockResolvedValue(undefined)
  Object.values(gitview).forEach((f) => f.mockReset().mockResolvedValue(undefined))
  settings.get.mockReset().mockResolvedValue(null)
  settings.set.mockReset().mockResolvedValue(undefined)
  gitview.changes.mockResolvedValue({ branch: 'main', head: 'x', conflicts: [], staged: [], unstaged: [], untracked: [], ahead: null })
  service.list.mockResolvedValue([session('a'), session('b')])
  service.meta.mockResolvedValue([])
  service.status.mockResolvedValue(status(true))
  projects.list.mockResolvedValue([])
  useUi.getState().reset()
})

describe('App', () => {
  it('opens the Settings page from the header gear and via Cmd+,', async () => {
    renderApp()
    fireEvent.click(await screen.findByRole('button', { name: 'Settings' }))
    expect(await screen.findByRole('region', { name: 'Settings' })).toBeTruthy()
    expect(useUi.getState().activeView).toBe('settings')

    act(() => useUi.getState().setActiveView('terminal'))
    fireEvent.keyDown(window, { key: ',', metaKey: true })
    expect(useUi.getState().activeView).toBe('settings')

    fireEvent.click(screen.getByRole('button', { name: 'Settings' }))
    expect(useUi.getState().activeView).toBe('terminal')
  })

  it('adds a project from the empty workspace', async () => {
    vi.mocked(open).mockResolvedValue('/w/one')
    projects.add.mockResolvedValue(project('p1', { root: '/w/one' }))
    renderApp()
    fireEvent.click(await screen.findByRole('button', { name: 'Choose a folder…' }))
    await waitFor(() => expect(projects.add).toHaveBeenCalledWith('/w/one', expect.anything()))
  })

  it('shows unassigned sessions without a workspace until one is opened', async () => {
    renderApp()
    expect((await screen.findAllByRole('button', { name: /title-/ })).length).toBe(2)
    expect(screen.getByText(/no projects yet/i)).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Choose a folder…' })).toBeTruthy()
    expect(log).toEqual([])

    fireEvent.click(screen.getByRole('button', { name: /title-b/ }))
    expect(log).toEqual(['new:b'])
    expect(useUi.getState().activeWorkspaceId).toBe('__unassigned__')
    expect(screen.getByRole('tab', { name: 'title-b' }).getAttribute('aria-selected')).toBe('true')
    expect(screen.getByRole('button', { name: 'Stop' })).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: /title-a/ }))
    expect(log).toEqual(['new:b', 'new:a'])
    expect(screen.getByRole('tab', { name: 'title-b' }).getAttribute('aria-selected')).toBe('false')
    expect(screen.getByRole('tab', { name: 'title-a' }).getAttribute('aria-selected')).toBe('true')
  })

  it('shows the banner and disables controls while disconnected, and recovers', async () => {
    renderApp()
    await vi.waitFor(() => expect(listeners()).toBe(6))
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
    await waitFor(() => expect(listeners()).toBe(6))

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
    await vi.waitFor(() => expect(listeners()).toBe(6))
    await screen.findAllByRole('button', { name: /title-/ })
    emit('session://updated', session('c'))
    expect(await screen.findByRole('button', { name: /title-c/ })).toBeTruthy()
  })

  it('stops the event sync on unmount', async () => {
    const { unmount } = renderApp()
    await vi.waitFor(() => expect(listeners()).toBe(6))
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
    await waitFor(() => expect(screen.getByRole('tab', { name: 'title-a' })).toBeTruthy())
    expect(screen.getByText(/one ›/)).toBeTruthy()
    expect(screen.getByText(/Claude Code ·/)).toBeTruthy()
  })

  it('creates the default shell once when a project workspace is activated', async () => {
    const p1 = project('p1', { name: 'one', root: '/w/one' })
    projects.list.mockResolvedValue([p1])
    service.list.mockResolvedValue([])
    service.create.mockResolvedValue(session('shell', { title: 'Shell' }))
    renderApp()

    const projectButton = await screen.findByRole('button', { name: /one.*\/w\/one/ })
    fireEvent.click(projectButton)
    await waitFor(() => expect(service.create).toHaveBeenCalledTimes(1))
    await screen.findByRole('tab', { name: 'Shell' })
    fireEvent.click(projectButton)

    expect(service.create).toHaveBeenCalledTimes(1)
    expect(service.create).toHaveBeenCalledWith(
      { agent: 'shell', projectId: 'p1', checkout: '/w/one', cols: 80, rows: 24, title: 'Shell' },
      expect.anything(),
    )
  })

  it('creates a worktree from Editor, switches to Terminal, and starts its first Shell tab there', async () => {
    const p1 = project('p1', { name: 'one', root: '/w/one' })
    const root = { name: 'one', path: p1.root, branch: 'main', head: '12345678', is_root: true, missing: false, dirty: false, changed: 0 }
    const created = { name: 'worktree', path: '/w/one-worktrees/worktree', branch: 'worktree', head: 'abcdef12', is_root: false, missing: false, dirty: false, changed: 0 }
    let createdWorktree = false
    projects.list.mockResolvedValue([p1])
    service.list.mockResolvedValue([])
    service.create.mockResolvedValue(session('shell', { title: 'Shell', cwd: created.path }))
    worktrees.list.mockImplementation(async () => ({ repo: true, worktrees: createdWorktree ? [root, created] : [root] }))
    worktrees.branches.mockResolvedValue({ branches: [{ name: 'main', checked_out_at: p1.root }], bases: [{ name: 'main', kind: 'local' }] })
    worktrees.create.mockImplementation(async () => { createdWorktree = true; return created })
    renderApp()

    fireEvent.keyDown(await screen.findByRole('button', { name: 'More actions for one' }), { key: 'ArrowDown' })
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Open Editor' }))
    fireEvent.keyDown(await screen.findByRole('button', { name: 'More actions for one' }), { key: 'ArrowDown' })
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Create worktree…' }))
    const createWorktree = await screen.findByRole('button', { name: 'Create worktree' })
    await waitFor(() => expect(createWorktree).toHaveProperty('disabled', false))
    fireEvent.click(createWorktree)
    await screen.findByRole('tab', { name: 'Shell' })

    expect(useUi.getState().activeWorkspaceId).toBe(`${p1.id}\u0000${created.path}`)
    expect(service.create).toHaveBeenCalledWith(
      { agent: 'shell', projectId: 'p1', checkout: created.path, cols: 80, rows: 24, title: 'Shell' },
      expect.anything(),
    )
  })

  it('keeps a created checkout without a phantom tab when its first Shell launch fails', async () => {
    const p1 = project('p1', { name: 'one', root: '/w/one' })
    const root = { name: 'one', path: p1.root, branch: 'main', head: '12345678', is_root: true, missing: false, dirty: false, changed: 0 }
    const created = { name: 'worktree', path: '/w/one-worktrees/worktree', branch: 'worktree', head: 'abcdef12', is_root: false, missing: false, dirty: false, changed: 0 }
    let createdWorktree = false
    projects.list.mockResolvedValue([p1])
    service.list.mockResolvedValue([])
    service.create.mockRejectedValue(new Error('shell launch failed'))
    worktrees.list.mockImplementation(async () => ({ repo: true, worktrees: createdWorktree ? [root, created] : [root] }))
    worktrees.branches.mockResolvedValue({ branches: [], bases: [{ name: 'main', kind: 'local' }] })
    worktrees.create.mockImplementation(async () => { createdWorktree = true; return created })
    renderApp()

    fireEvent.keyDown(await screen.findByRole('button', { name: 'More actions for one' }), { key: 'ArrowDown' })
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Create worktree…' }))
    const createWorktree = await screen.findByRole('button', { name: 'Create worktree' })
    await waitFor(() => expect(createWorktree).toHaveProperty('disabled', false))
    fireEvent.click(createWorktree)

    expect((await screen.findByRole('alert')).textContent).toMatch(/shell launch failed/)
    expect(screen.queryByRole('tab')).toBeNull()
    expect(useUi.getState().activeWorkspaceId).toBe(`${p1.id}\u0000${created.path}`)
    expect(worktrees.create).toHaveBeenCalled()
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
    expect(service.create).toHaveBeenCalledWith({ agent: 'shell', projectId: 'p1', checkout: '/w/one', cols: 80, rows: 24 }, expect.anything())
    expect(useUi.getState().selectedId).toBe('new')
  })

  it('removing a project never stops its sessions', async () => {
    const p1 = project('p1', { name: 'one', root: '/w/one' })
    projects.list.mockResolvedValueOnce([p1]).mockResolvedValue([])
    service.meta.mockResolvedValue([meta('a', { project_id: 'p1' })])
    projects.remove.mockResolvedValue(undefined)
    renderApp()
    await screen.findByRole('button', { name: 'More actions for one' })

    fireEvent.keyDown(screen.getByRole('button', { name: 'More actions for one' }), { key: 'ArrowDown' })
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Remove project' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Remove' }))
    await waitFor(() => expect(screen.getByText('Unassigned')).toBeTruthy()) // a survives, unassigned
    expect(sessionButtons()).toHaveLength(2)
    expect(service.stop).not.toHaveBeenCalled()
  })

  it('keeps the terminal workspace mounted while the Editor covers it', async () => {
    const p1 = project('p1', { name: 'one', root: '/w/one' })
    const root = { name: 'one', path: p1.root, branch: 'main', head: '12345678', is_root: true, missing: false, dirty: false, changed: 0 }
    projects.list.mockResolvedValue([p1])
    service.list.mockResolvedValue([])
    service.create.mockResolvedValue(session('shell', { title: 'Shell' }))
    worktrees.list.mockResolvedValue({ repo: true, worktrees: [root] })
    renderApp()

    fireEvent.click(await screen.findByRole('button', { name: /one.*\/w\/one/ }))
    await screen.findByRole('tab', { name: 'Shell' })
    expect(log).toEqual(['new:shell'])

    fireEvent.keyDown(await screen.findByRole('button', { name: 'More actions for one' }), { key: 'ArrowDown' })
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Open Editor' }))
    const editor = await screen.findByRole('region', { name: 'one editor' })
    // The terminal section stays mounted (hidden in the browser; jsdom does not
    // apply the `hidden` class), so its tab and controller survive the switch.
    expect(document.querySelector('section[aria-label="one terminal workspace"]')).toBeTruthy()
    expect(screen.queryByRole('tab', { name: 'Shell' })).toBeTruthy()
    expect(log).toEqual(['new:shell'])

    fireEvent.click(within(editor).getByRole('button', { name: 'Terminal' }))
    // The same tab and controller come back — nothing was remounted or disposed.
    await screen.findByRole('tab', { name: 'Shell' })
    expect(log).toEqual(['new:shell'])
  })

  it('keeps the resized project sidebar width across Terminal and Editor switches', async () => {
    const p1 = project('p1', { name: 'one', root: '/w/one' })
    const root = { name: 'one', path: p1.root, branch: 'main', head: '12345678', is_root: true, missing: false, dirty: false, changed: 0 }
    projects.list.mockResolvedValue([p1])
    service.list.mockResolvedValue([])
    service.create.mockResolvedValue(session('shell', { title: 'Shell' }))
    worktrees.list.mockResolvedValue({ repo: true, worktrees: [root] })
    renderApp()

    fireEvent.click(await screen.findByRole('button', { name: /one.*\/w\/one/ }))
    await screen.findByRole('tab', { name: 'Shell' })
    const separator = screen.getByRole('separator', { name: 'Resize project sidebar' })
    expect(separator.getAttribute('aria-valuenow')).toBe('320')
    expect(document.querySelector('aside')!.style.width).toBe('320px')

    fireEvent.keyDown(separator, { key: 'ArrowRight' })
    expect(separator.getAttribute('aria-valuenow')).toBe('336')
    expect(useUi.getState().panelWidths.projectTree).toBe(336)

    fireEvent.keyDown(await screen.findByRole('button', { name: 'More actions for one' }), { key: 'ArrowDown' })
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Open Editor' }))
    const editor = await screen.findByRole('region', { name: 'one editor' })
    expect(useUi.getState().panelWidths.projectTree).toBe(336)

    fireEvent.click(within(editor).getByRole('button', { name: 'Terminal' }))
    await screen.findByRole('tab', { name: 'Shell' })
    expect(separator.getAttribute('aria-valuenow')).toBe('336')
    expect(document.querySelector('aside')!.style.width).toBe('336px')
  })
})
