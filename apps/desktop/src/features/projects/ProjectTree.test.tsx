import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient, queryKeys } from '../../queryClient'
import type { AgentInfo, Project, SessionInfo, SessionMeta, WorktreeInfo } from '../../services/types'
import { checkoutKey, useUi } from '../../stores/ui'
import { ProjectTree } from './ProjectTree'

const { projects, session, agents, open, worktrees } = vi.hoisted(() => ({
  projects: { list: vi.fn(), add: vi.fn(), remove: vi.fn() },
  session: { list: vi.fn(), meta: vi.fn(), create: vi.fn(), status: vi.fn() },
  agents: { list: vi.fn() },
  open: vi.fn(),
  worktrees: { list: vi.fn(), branches: vi.fn(), create: vi.fn(), remove: vi.fn(), envCandidates: vi.fn(), envCopy: vi.fn() },
}))
vi.mock('../../services/projects', () => ({ projectService: projects }))
vi.mock('../../services/session', () => ({ sessionService: session }))
vi.mock('../../services/agents', () => ({ agentService: agents }))
vi.mock('../../services/worktrees', () => ({ worktreeService: worktrees }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open }))

const AGENTS: AgentInfo[] = [
  { id: 'shell', label: 'Shell', available: true, path: '/bin/zsh' },
  { id: 'claude', label: 'Claude Code', available: true, path: '/usr/local/bin/claude' },
  { id: 'codex', label: 'Codex', available: false, path: null },
]

const project = (id: string, over: Partial<Project> = {}): Project => ({
  id,
  name: `name-${id}`,
  root: `/work/${id}`,
  created_at: 1,
  missing: false,
  ...over,
})
const makeSession = (id: string): SessionInfo => ({
  id,
  title: `title-${id}`,
  argv: ['sh'],
  cwd: '/tmp',
  state: 'running',
  exit_code: null,
  error: null,
  started_at: 1,
  ended_at: null,
})
const makeWorktree = (path: string, over: Partial<WorktreeInfo> = {}): WorktreeInfo => ({
  name: path.split('/').at(-1)!,
  path,
  branch: 'main',
  head: '1234567890abcdef',
  is_root: false,
  missing: false,
  dirty: false,
  changed: 0,
  ...over,
})
const meta = (sessionId: string, projectId: string | null, agent = 'shell'): SessionMeta => ({
  session_id: sessionId,
  project_id: projectId,
  agent,
  argv: ['zsh'],
  cwd: '/tmp',
  title: '',
  created_at: 1,
})

let qc: QueryClient
const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>
const renderTree = () => render(<ProjectTree />, { wrapper })
/** Project activation and disclosure are separate controls. */
const projectButton = (id: string) => screen.getByRole('button', { name: new RegExp(`^name-${id}\\s*/work/${id}$`) })
const toggleOf = (id: string) => screen.getByRole('button', { name: new RegExp(`^(Expand|Collapse) name-${id} sessions$`) })
/** Opens the `＋` menu of a project the way a keyboard user does. */
const openMenu = (name: string) => fireEvent.keyDown(screen.getByRole('button', { name: `New session in ${name}` }), { key: 'ArrowDown' })

beforeEach(() => {
  qc = makeQueryClient()
  Object.values(projects).forEach((f) => f.mockReset().mockResolvedValue([]))
  Object.values(session).forEach((f) => f.mockReset().mockResolvedValue([]))
  session.status.mockResolvedValue({ connected: true, default_shell: '/bin/zsh', home: '/h' })
  agents.list.mockReset().mockResolvedValue(AGENTS)
  worktrees.list.mockReset().mockResolvedValue({ repo: false, worktrees: [] })
  worktrees.branches.mockReset().mockResolvedValue({ branches: [], bases: [] })
  worktrees.create.mockReset()
  worktrees.remove.mockReset().mockResolvedValue(undefined)
  worktrees.envCandidates.mockReset().mockResolvedValue([])
  worktrees.envCopy.mockReset().mockResolvedValue({ copied: [], skipped: [], failed: [] })
  open.mockReset().mockResolvedValue(null)
  useUi.getState().reset()
})

describe('tree', () => {
  it('nests sessions under their project, with agent labels and counts; the rest go to Unassigned', async () => {
    projects.list.mockResolvedValue([project('p1'), project('p2')])
    session.list.mockResolvedValue([makeSession('a'), makeSession('b'), makeSession('c'), makeSession('d')])
    session.meta.mockResolvedValue([
      meta('a', 'p1', 'claude'),
      meta('b', 'p1', 'custom'),
      meta('c', null),
      // d has no metadata row; a session pointing at a removed project is also unassigned
    ])
    renderTree()
    await screen.findByRole('button', { name: /title-a/ })

    const p1 = projectButton('p1').closest('li')!
    expect(within(p1).getByLabelText('2 sessions')).toBeTruthy()
    expect(within(p1).getByRole('button', { name: /Claude Code.*title-a/ })).toBeTruthy()
    expect(within(p1).getByRole('button', { name: /Custom.*title-b/ })).toBeTruthy()
    const p2 = projectButton('p2').closest('li')!
    expect(within(p2).getByLabelText('0 sessions')).toBeTruthy()
    expect(within(p2).queryByRole('list')).toBeNull()

    const unassigned = screen.getByText('Unassigned').closest('li')!
    expect(within(unassigned).getByLabelText('2 sessions')).toBeTruthy()
    expect(within(unassigned).getByRole('button', { name: /title-c/ })).toBeTruthy()
    expect(within(unassigned).getByRole('button', { name: /title-d/ })).toBeTruthy()
  })

  it('hides Unassigned when empty and shows the empty state with nothing at all', async () => {
    renderTree()
    await screen.findByText('No projects yet')
    expect(screen.queryByText('Unassigned')).toBeNull()
  })

  it('collapses and expands a project without selecting anything', async () => {
    projects.list.mockResolvedValue([project('p1')])
    session.list.mockResolvedValue([makeSession('a')])
    session.meta.mockResolvedValue([meta('a', 'p1')])
    renderTree()
    await screen.findByRole('button', { name: /title-a/ })

    const toggle = toggleOf('p1')
    expect(toggle.getAttribute('aria-expanded')).toBe('true')
    fireEvent.click(toggle)
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    expect(screen.queryByRole('button', { name: /title-a/ })).toBeNull()
    fireEvent.click(toggle)
    expect(screen.getByRole('button', { name: /title-a/ })).toBeTruthy()
    expect(useUi.getState().selectedId).toBeNull()
    expect(useUi.getState().activeWorkspaceId).toBeNull()
  })

  it('clicking a session selects it; selecting inside a collapsed project reopens it', async () => {
    projects.list.mockResolvedValue([project('p1')])
    session.list.mockResolvedValue([makeSession('a')])
    session.meta.mockResolvedValue([meta('a', 'p1')])
    renderTree()
    fireEvent.click(await screen.findByRole('button', { name: /title-a/ }))
    expect(useUi.getState().selectedId).toBe('a')
    expect(screen.getByRole('button', { name: /title-a/ }).getAttribute('aria-current')).toBe('true')

    const toggle = toggleOf('p1')
    fireEvent.click(toggle)
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    act(() => useUi.getState().select(null))
    act(() => useUi.getState().select('a'))
    await waitFor(() => expect(toggle.getAttribute('aria-expanded')).toBe('true'))
    expect(useUi.getState().activeWorkspaceId).toBe(checkoutKey('p1', '/work/p1'))
  })

  it('shows the missing badge and disables ＋ for a vanished root, but it can still be removed', async () => {
    projects.list.mockResolvedValue([project('gone', { missing: true })])
    renderTree()
    await screen.findByText('missing')
    expect(screen.getByRole('button', { name: 'New session in name-gone' })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: 'Remove name-gone' })).toHaveProperty('disabled', false)
  })

  it('disables ＋ while the daemon is disconnected, but Remove stays usable', async () => {
    projects.list.mockResolvedValue([project('p1')])
    session.status.mockResolvedValue({ connected: false, default_shell: '/bin/zsh', home: '/h' })
    renderTree()
    const plus = await screen.findByRole('button', { name: 'New session in name-p1' })
    await waitFor(() => expect(plus).toHaveProperty('disabled', true))
    expect(screen.getByRole('button', { name: 'Remove name-p1' })).toHaveProperty('disabled', false)
    expect(projectButton('p1')).toHaveProperty('disabled', false)
  })

  it('disables an open agent menu when the daemon disconnects', async () => {
    projects.list.mockResolvedValue([project('p1')])
    renderTree()
    await screen.findByRole('button', { name: 'New session in name-p1' })
    openMenu('name-p1')
    const claude = await screen.findByRole('menuitem', { name: 'Claude Code' })
    const custom = screen.getByRole('menuitem', { name: 'Custom command…' })

    act(() => qc.setQueryData(queryKeys.daemonStatus, { connected: false, default_shell: '/bin/zsh', home: '/h' }))

    await waitFor(() => expect(claude.getAttribute('aria-disabled')).toBe('true'))
    expect(custom.getAttribute('aria-disabled')).toBe('true')
    fireEvent.click(claude)
    expect(session.create).not.toHaveBeenCalled()
  })

  it('disables an open custom command form when the daemon disconnects', async () => {
    projects.list.mockResolvedValue([project('p1')])
    renderTree()
    await screen.findByRole('button', { name: 'New session in name-p1' })
    openMenu('name-p1')
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Custom command…' }))
    await screen.findByLabelText('Command')

    act(() => qc.setQueryData(queryKeys.daemonStatus, { connected: false, default_shell: '/bin/zsh', home: '/h' }))

    const start = screen.getByRole('button', { name: 'Start' })
    await waitFor(() => expect(start).toHaveProperty('disabled', true))
    fireEvent.click(start)
    expect(session.create).not.toHaveBeenCalled()
  })

  it('a rejected project list keeps the note and still lists daemon sessions under Unassigned', async () => {
    projects.list.mockRejectedValue({ code: 'storage', message: 'cannot open goko.db', hint: null })
    session.list.mockResolvedValue([makeSession('a')])
    session.meta.mockResolvedValue([meta('a', 'p1')]) // claims a project, but the list never resolves
    renderTree()
    expect((await screen.findByText('cannot open goko.db')).closest('[role="alert"]')).toBeTruthy()

    const unassigned = screen.getByText('Unassigned').closest('li')!
    fireEvent.click(within(unassigned).getByRole('button', { name: /title-a/ }))
    expect(useUi.getState().selectedId).toBe('a')
  })

  it('a rejected metadata list leaves every session under Unassigned', async () => {
    projects.list.mockResolvedValue([project('p1')])
    session.list.mockResolvedValue([makeSession('a')])
    session.meta.mockRejectedValue({ code: 'storage', message: 'cannot open goko.db', hint: null })
    renderTree()

    const unassigned = (await screen.findByText('Unassigned')).closest('li')!
    expect(within(unassigned).getByRole('button', { name: /title-a/ })).toBeTruthy()
    expect(within(toggleOf('p1').closest('li')!).getByLabelText('0 sessions')).toBeTruthy()
  })
})

describe('new session', () => {
  beforeEach(() => {
    projects.list.mockResolvedValue([project('p1')])
  })

  it('lists every agent, with the missing one disabled and noted', async () => {
    renderTree()
    await screen.findByRole('button', { name: 'New session in name-p1' })
    openMenu('name-p1')

    const items = await screen.findAllByRole('menuitem')
    expect(items.map((i) => i.textContent)).toEqual(['Shell', 'Claude Code', 'Codex — not found on PATH', 'Custom command…'])
    expect(items[2].getAttribute('aria-disabled')).toBe('true')
  })

  it('picking an agent starts it in the project and selects the new session', async () => {
    session.create.mockResolvedValue(makeSession('new'))
    renderTree()
    await screen.findByRole('button', { name: 'New session in name-p1' })
    openMenu('name-p1')
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Claude Code' }))

    await waitFor(() => expect(useUi.getState().selectedId).toBe('new'))
    expect(session.create).toHaveBeenCalledWith(
      { agent: 'claude', projectId: 'p1', checkout: '/work/p1', cols: 80, rows: 24 },
      expect.anything(),
    )
  })

  it('the new session appears under its project once the metadata lands, not under Unassigned', async () => {
    session.list.mockResolvedValue([])
    session.meta.mockResolvedValueOnce([]).mockResolvedValue([meta('new', 'p1', 'claude')])
    session.create.mockResolvedValue(makeSession('new'))
    renderTree()
    await screen.findByRole('button', { name: 'New session in name-p1' })
    openMenu('name-p1')
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Claude Code' }))

    const p1 = toggleOf('p1').closest('li')!
    expect(await within(p1).findByRole('button', { name: /title-new/ })).toBeTruthy()
    expect(screen.queryByText('Unassigned')).toBeNull()
  })

  it('custom command opens an inline form prefilled with the default shell and starts the typed command', async () => {
    session.create.mockResolvedValue(makeSession('new'))
    renderTree()
    await screen.findByRole('button', { name: 'New session in name-p1' })
    openMenu('name-p1')
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Custom command…' }))

    const input = (await screen.findByLabelText('Command')) as HTMLInputElement
    await waitFor(() => expect(input.value).toBe('/bin/zsh'))
    fireEvent.change(input, { target: { value: 'htop -d 5' } })
    fireEvent.click(screen.getByRole('button', { name: 'Start' }))

    await waitFor(() =>
      expect(session.create).toHaveBeenCalledWith(
        { command: 'htop -d 5', projectId: 'p1', checkout: '/work/p1', cols: 80, rows: 24 },
        expect.anything(),
      ),
    )
    await waitFor(() => expect(screen.queryByLabelText('Command')).toBeNull())
  })

  it('keeps the custom form and shows the error when the create is rejected', async () => {
    session.create.mockRejectedValue({ code: 'invalid_argument', message: 'bad quote', hint: 'close the quote' })
    renderTree()
    await screen.findByRole('button', { name: 'New session in name-p1' })
    openMenu('name-p1')
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Custom command…' }))
    await waitFor(() => expect((screen.getByLabelText('Command') as HTMLInputElement).value).toBe('/bin/zsh'))
    fireEvent.click(screen.getByRole('button', { name: 'Start' }))

    expect((await screen.findByText('bad quote')).closest('[role="alert"]')).toBeTruthy()
    expect(screen.getByText('close the quote')).toBeTruthy()
    expect(screen.getByLabelText('Command')).toBeTruthy()
  })
})

describe('worktrees', () => {
  const root = makeWorktree('/work/p1', { name: 'p1', branch: 'main', is_root: true })
  const feature = makeWorktree('/work/p1-worktrees/feature', { branch: 'feature' })

  beforeEach(() => {
    projects.list.mockResolvedValue([project('p1')])
  })

  it('groups sessions by their checkout and starts a session in that checkout', async () => {
    worktrees.list.mockResolvedValue({ repo: true, worktrees: [root, feature] })
    session.list.mockResolvedValue([
      { ...makeSession('root-session'), cwd: '/work/p1/src' },
      { ...makeSession('feature-session'), cwd: '/work/p1-worktrees/feature/src' },
    ])
    session.meta.mockResolvedValue([meta('root-session', 'p1'), meta('feature-session', 'p1', 'claude')])
    session.create.mockResolvedValue(makeSession('new-feature'))

    renderTree()
    await screen.findByRole('button', { name: /feature.*feature.*\/work\/p1-worktrees\/feature/ })
    const featureRow = screen.getByText('feature').closest('li')!
    expect(within(featureRow).getByLabelText('1 sessions')).toBeTruthy()
    expect(within(featureRow).getByRole('button', { name: /Claude Code.*title-feature-session/ })).toBeTruthy()
    expect(within(projectButton('p1').closest('li')!).getByRole('button', { name: /Shell.*title-root-session/ })).toBeTruthy()

    fireEvent.keyDown(await within(featureRow).findByRole('button', { name: 'New session in name-p1 · feature' }), { key: 'ArrowDown' })
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Claude Code' }))
    await waitFor(() => expect(useUi.getState().activeWorkspaceId).toBe(checkoutKey('p1', feature.path)))
    expect(session.create).toHaveBeenCalledWith(
      { agent: 'claude', projectId: 'p1', checkout: feature.path, cols: 80, rows: 24 },
      expect.anything(),
    )
  })

  it('blocks removal for changed checkouts and for sessions that have already exited', async () => {
    const changed = makeWorktree('/work/p1-worktrees/changed', { branch: 'changed', dirty: true, changed: 2 })
    const used = makeWorktree('/work/p1-worktrees/used', { branch: 'used' })
    worktrees.list.mockResolvedValue({ repo: true, worktrees: [root, changed, used] })
    session.list.mockResolvedValue([{ ...makeSession('exited'), cwd: `${used.path}/src`, state: 'exited' }])
    session.meta.mockResolvedValue([meta('exited', 'p1')])

    renderTree()
    const changedRemove = await screen.findByRole('button', { name: /Cannot remove changed: Commit or discard/ })
    const usedRemove = screen.getByRole('button', { name: /Cannot remove used: Close all sessions/ })
    expect(changedRemove.getAttribute('aria-disabled')).toBe('true')
    expect(usedRemove.getAttribute('aria-disabled')).toBe('true')
  })

  it('blocks removal for an exited session attached from another project without rehoming it', async () => {
    projects.list.mockResolvedValue([project('p1'), project('p2')])
    worktrees.list.mockImplementation((id: string) => Promise.resolve(id === 'p1'
      ? { repo: true, worktrees: [root, feature] }
      : { repo: false, worktrees: [] }))
    session.list.mockResolvedValue([{ ...makeSession('external'), cwd: `${feature.path}/src`, state: 'exited' }])
    session.meta.mockResolvedValue([meta('external', 'p2')])

    renderTree()

    const remove = await screen.findByRole('button', {
      name: /Cannot remove feature: Close all sessions in this checkout first/,
    })
    expect(remove.getAttribute('aria-disabled')).toBe('true')
    expect(within(screen.getByText('feature').closest('li')!).getByLabelText('1 sessions')).toBeTruthy()
    expect(within(projectButton('p2').closest('li')!).getByRole('button', { name: /title-external/ })).toBeTruthy()
    expect(within(screen.getByText('feature').closest('li')!).queryByRole('button', { name: /title-external/ })).toBeNull()
  })

  it('blocks worktree removal while any session has an unknown current directory', async () => {
    projects.list.mockResolvedValue([project('p1'), project('p2')])
    worktrees.list.mockImplementation((id: string) => Promise.resolve(id === 'p1'
      ? { repo: true, worktrees: [root, feature] }
      : { repo: false, worktrees: [] }))
    session.list.mockResolvedValue([{ ...makeSession('unknown-cwd'), cwd: '', state: 'exited' }])
    session.meta.mockResolvedValue([meta('unknown-cwd', 'p2')])

    renderTree()

    const remove = await screen.findByRole('button', {
      name: /Cannot remove feature: Goko cannot confirm the current directory of every session/,
    })
    expect(remove.getAttribute('aria-disabled')).toBe('true')
    fireEvent.focus(remove)
    expect((await screen.findByRole('tooltip')).textContent?.includes('close them first')).toBe(true)
    fireEvent.click(remove)
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('blocks removal while a session in another project is running', async () => {
    projects.list.mockResolvedValue([project('p1'), project('p2')])
    worktrees.list.mockImplementation((id: string) => Promise.resolve(id === 'p1'
      ? { repo: true, worktrees: [root, feature] }
      : { repo: false, worktrees: [] }))
    session.list.mockResolvedValue([{ ...makeSession('other-live'), cwd: '/work/p2', state: 'running' }])
    session.meta.mockResolvedValue([meta('other-live', 'p2')])

    renderTree()

    const remove = await screen.findByRole('button', {
      name: /Cannot remove feature: Close all running Goko sessions/,
    })
    expect(remove.getAttribute('aria-disabled')).toBe('true')
  })

  it('keeps a missing checkout and its sessions visible while disabling launches and removal', async () => {
    const missing = makeWorktree('/work/p1-worktrees/gone', { branch: 'gone', missing: true, dirty: null, changed: null })
    worktrees.list.mockResolvedValue({ repo: true, worktrees: [root, missing] })
    session.list.mockResolvedValue([{ ...makeSession('old-session'), cwd: `${missing.path}/src`, state: 'exited' }])
    session.meta.mockResolvedValue([meta('old-session', 'p1')])

    renderTree()
    const row = (await screen.findByText('gone')).closest('li')!
    expect(within(row).getByText('missing')).toBeTruthy()
    expect(within(row).getByRole('button', { name: 'New session in name-p1 · gone' })).toHaveProperty('disabled', true)
    expect(within(row).getByRole('button', { name: /Cannot remove gone: This checkout is missing/ }).getAttribute('aria-disabled')).toBe('true')
    expect(within(row).getByRole('button', { name: /title-old-session/ })).toBeTruthy()
  })

  it('keeps sessions that match no listed checkout directly under the project', async () => {
    worktrees.list.mockResolvedValue({ repo: true, worktrees: [root, feature] })
    session.list.mockResolvedValue([{ ...makeSession('outside'), cwd: '/elsewhere/project-copy' }])
    session.meta.mockResolvedValue([meta('outside', 'p1')])
    renderTree()

    const other = (await screen.findByText('Other sessions')).closest('div')!
    expect(within(other.parentElement!).getByRole('button', { name: /title-outside/ })).toBeTruthy()
  })

  it('refreshes the checkout list on demand', async () => {
    worktrees.list.mockResolvedValue({ repo: true, worktrees: [root, feature] })
    renderTree()
    await screen.findByText('feature')
    const callsBeforeRefresh = worktrees.list.mock.calls.length
    fireEvent.click(screen.getByRole('button', { name: 'Refresh worktrees for name-p1' }))
    await waitFor(() => expect(worktrees.list.mock.calls.length).toBeGreaterThan(callsBeforeRefresh))
  })

  it('returns focus to the create action after the dialog closes', async () => {
    worktrees.list.mockResolvedValue({ repo: true, worktrees: [root] })
    worktrees.branches.mockResolvedValue({ branches: [], bases: [{ name: 'main', kind: 'local' }] })
    renderTree()
    const trigger = await screen.findByRole('button', { name: 'Create worktree in name-p1' })
    fireEvent.click(trigger)
    await screen.findByRole('dialog', { name: 'New worktree' })
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(document.activeElement).toBe(trigger))
  })

  it('confirms the exact path, warns that the branch stays, and removes a clean unused worktree', async () => {
    worktrees.list.mockImplementation(() => Promise.resolve({ repo: true, worktrees: worktrees.remove.mock.calls.length ? [root] : [root, feature] }))
    renderTree()
    await screen.findByText('feature')
    await waitFor(() => expect(screen.getByRole('button', { name: 'Remove feature' })).toHaveProperty('disabled', false))
    act(() => useUi.getState().activateWorkspace(checkoutKey('p1', feature.path)))
    fireEvent.click(screen.getByRole('button', { name: 'Remove feature' }))
    expect(screen.getByText(feature.path)).toBeTruthy()
    expect(screen.getByText(/the branch is preserved/i)).toBeTruthy()
    expect(screen.getByText(/ignored local files/i)).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Remove feature' })))
    fireEvent.click(screen.getByRole('button', { name: 'Remove feature' }))
    fireEvent.click(screen.getByRole('button', { name: 'Remove worktree' }))

    await waitFor(() => expect(worktrees.remove).toHaveBeenCalledWith('p1', feature.path))
    await waitFor(() => expect(screen.queryByText('feature')).toBeNull())
    expect(useUi.getState().activeWorkspaceId).toBe(checkoutKey('p1', project('p1').root))
    await waitFor(() => expect(document.activeElement).toBe(projectButton('p1')))
  })

  it('disables new sessions in a worktree while its removal is in progress', async () => {
    let finishRemoval!: () => void
    worktrees.remove.mockImplementation(() => new Promise<void>((resolve) => { finishRemoval = resolve }))
    worktrees.list.mockResolvedValue({ repo: true, worktrees: [root, feature] })
    renderTree()
    await screen.findByText('feature')
    await waitFor(() => expect(screen.getByRole('button', { name: 'Remove feature' })).toHaveProperty('disabled', false))
    fireEvent.click(screen.getByRole('button', { name: 'Remove feature' }))
    fireEvent.click(screen.getByRole('button', { name: 'Remove worktree' }))
    await waitFor(() => expect(worktrees.remove).toHaveBeenCalledWith('p1', feature.path))
    const launch = document.querySelector<HTMLButtonElement>('button[aria-label="New session in name-p1 · feature"]')
    expect(launch).toHaveProperty('disabled', true)
    act(() => finishRemoval())
    await waitFor(() => expect(screen.getByRole('button', { name: 'Remove feature' })).toHaveProperty('disabled', false))
  })

  it('blocks removal while the daemon is disconnected and cannot verify sessions', async () => {
    worktrees.list.mockResolvedValue({ repo: true, worktrees: [root, feature] })
    session.status.mockResolvedValue({ connected: false, default_shell: '/bin/zsh', home: '/h' })
    renderTree()

    const remove = await screen.findByRole('button', { name: /Cannot remove feature: The daemon is disconnected/ })
    expect(remove.getAttribute('aria-disabled')).toBe('true')
  })

  it('explains when an older daemon cannot safely remove worktrees', async () => {
    worktrees.list.mockResolvedValue({ repo: true, worktrees: [root, feature] })
    session.status.mockResolvedValue({ connected: true, live_cwd_supported: false, default_shell: '/bin/zsh', home: '/h' })

    renderTree()

    const remove = await screen.findByRole('button', {
      name: /Cannot remove feature: Update and restart the connected gokod manually/,
    })
    expect(remove.getAttribute('aria-disabled')).toBe('true')
  })

  it('creates and activates a checkout, then lets the user retry a failed env copy', async () => {
    const created = makeWorktree('/work/p1-worktrees/try-me', { branch: 'try-me' })
    worktrees.list.mockResolvedValue({ repo: true, worktrees: [root] })
    worktrees.branches.mockResolvedValue({ branches: [], bases: [{ name: 'main', kind: 'local' }] })
    worktrees.create.mockResolvedValue(created)
    worktrees.envCandidates.mockResolvedValue([{ name: '.env', eligible: true, reason: null }])
    worktrees.envCopy
      .mockResolvedValueOnce({ copied: [], skipped: [], failed: [{ name: '.env', error: 'permission denied' }] })
      .mockResolvedValueOnce({ copied: ['.env'], skipped: [], failed: [] })

    renderTree()
    fireEvent.click(await screen.findByRole('button', { name: 'Create worktree in name-p1' }))
    fireEvent.change(await screen.findByLabelText('Name'), { target: { value: 'try-me' } })
    fireEvent.click(screen.getByRole('checkbox', { name: /Copy environment files into this worktree/ }))
    fireEvent.click(screen.getByRole('button', { name: 'Create worktree' }))

    expect(await screen.findByText('.env: permission denied')).toBeTruthy()
    expect(useUi.getState().activeWorkspaceId).toBe(checkoutKey('p1', created.path))
    fireEvent.click(screen.getByRole('button', { name: 'Retry copy' }))
    await waitFor(() => expect(worktrees.envCopy).toHaveBeenCalledTimes(2))
    expect(await screen.findByText('.env: copied')).toBeTruthy()
    expect(worktrees.envCopy).toHaveBeenLastCalledWith('p1', '/work/p1', created.path)
  })

  it('opens the Editor on the root checkout and on a linked worktree checkout', async () => {
    worktrees.list.mockResolvedValue({ repo: true, worktrees: [root, feature] })
    renderTree()
    fireEvent.click(await screen.findByRole('button', { name: 'Open Editor for feature' }))
    expect(useUi.getState().activeWorkspaceId).toBe(checkoutKey('p1', feature.path))
    expect(useUi.getState().activeView).toBe('editor')

    fireEvent.click(screen.getByRole('button', { name: 'Open Editor for name-p1' }))
    expect(useUi.getState().activeWorkspaceId).toBe(checkoutKey('p1', root.path))
    expect(useUi.getState().activeView).toBe('editor')
  })

  it('hides the Editor action for non-Git projects', async () => {
    worktrees.list.mockResolvedValue({ repo: false, worktrees: [] })
    renderTree()
    await screen.findByRole('button', { name: /^name-p1\s*\/work\/p1$/ })
    expect(screen.queryByRole('button', { name: /Open Editor/ })).toBeNull()
  })
})

describe('add', () => {
  it('calls add with the picked path and refetches the projects', async () => {
    open.mockResolvedValue('/picked/dir')
    projects.add.mockResolvedValue(project('new', { root: '/picked/dir' }))
    renderTree()
    await screen.findByText('No projects yet')

    fireEvent.click(screen.getByRole('button', { name: 'Add Project' }))
    await waitFor(() => expect(projects.add).toHaveBeenCalledWith('/picked/dir', expect.anything()))
    await waitFor(() => expect(projects.list).toHaveBeenCalledTimes(2))
  })

  it('a cancelled dialog changes nothing', async () => {
    renderTree()
    await screen.findByText('No projects yet')
    fireEvent.click(screen.getByRole('button', { name: 'Add Project' }))
    await waitFor(() => expect(open).toHaveBeenCalled())
    expect(projects.add).not.toHaveBeenCalled()
  })
})

describe('remove', () => {
  it('takes the two-step confirm and invalidates projects and sessionMeta', async () => {
    projects.list.mockResolvedValueOnce([project('p1', { name: 'doomed' })]).mockResolvedValueOnce([])
    renderTree()
    await screen.findByText('doomed')

    fireEvent.click(screen.getByRole('button', { name: 'Remove doomed' }))
    expect(projects.remove).not.toHaveBeenCalled()
    expect(screen.getByText('Remove doomed?')).toBeTruthy()
    expect(screen.getByText(/files and running sessions are untouched/i)).toBeTruthy()
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Cancel' })))

    // Cancel once to prove it is two-step, then confirm.
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Remove doomed' })))
    expect(screen.queryByText('Remove doomed?')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Remove doomed' }))
    fireEvent.click(screen.getByRole('button', { name: 'Remove' }))

    await waitFor(() => expect(projects.remove).toHaveBeenCalledWith('p1', expect.anything()))
    await waitFor(() => expect(screen.queryByText('doomed')).toBeNull())
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Add Project' }))
    expect(projects.list).toHaveBeenCalledTimes(2)
    expect(session.meta).toHaveBeenCalledTimes(2)
  })

  it('its sessions keep running and fall to Unassigned', async () => {
    projects.list.mockResolvedValueOnce([project('p1')]).mockResolvedValue([])
    session.list.mockResolvedValue([makeSession('a')])
    session.meta.mockResolvedValueOnce([meta('a', 'p1')]).mockResolvedValue([meta('a', null)])
    renderTree()
    await screen.findByRole('button', { name: /title-a/ })

    fireEvent.click(screen.getByRole('button', { name: 'Remove name-p1' }))
    fireEvent.click(screen.getByRole('button', { name: 'Remove' }))
    await waitFor(() => expect(screen.getByText('Unassigned')).toBeTruthy())
    expect(screen.getByRole('button', { name: /title-a/ })).toBeTruthy()
    expect(session.list).toHaveBeenCalledTimes(1)
  })
})
