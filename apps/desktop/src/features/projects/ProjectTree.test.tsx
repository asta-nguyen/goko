import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient } from '../../queryClient'
import type { AgentInfo, Project, SessionInfo, SessionMeta } from '../../services/types'
import { useUi } from '../../stores/ui'
import { ProjectTree } from './ProjectTree'

const { projects, session, agents, open } = vi.hoisted(() => ({
  projects: { list: vi.fn(), add: vi.fn(), remove: vi.fn() },
  session: { list: vi.fn(), meta: vi.fn(), create: vi.fn(), status: vi.fn() },
  agents: { list: vi.fn() },
  open: vi.fn(),
}))
vi.mock('../../services/projects', () => ({ projectService: projects }))
vi.mock('../../services/session', () => ({ sessionService: session }))
vi.mock('../../services/agents', () => ({ agentService: agents }))
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
/** The project's toggle row: its accessible name is the name followed by the root. */
const toggleOf = (id: string) => screen.getByRole('button', { name: new RegExp(`^name-${id}\\s*/work/${id}$`) })
/** Opens the `＋` menu of a project the way a keyboard user does. */
const openMenu = (name: string) => fireEvent.keyDown(screen.getByRole('button', { name: `New session in ${name}` }), { key: 'ArrowDown' })

beforeEach(() => {
  qc = makeQueryClient()
  Object.values(projects).forEach((f) => f.mockReset().mockResolvedValue([]))
  Object.values(session).forEach((f) => f.mockReset().mockResolvedValue([]))
  session.status.mockResolvedValue({ connected: true, default_shell: '/bin/zsh', home: '/h' })
  agents.list.mockReset().mockResolvedValue(AGENTS)
  open.mockReset().mockResolvedValue(null)
  useUi.setState({ selectedId: null })
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

    const p1 = toggleOf('p1').closest('li')!
    expect(within(p1).getByLabelText('2 sessions')).toBeTruthy()
    expect(within(p1).getByRole('button', { name: /Claude Code.*title-a/ })).toBeTruthy()
    expect(within(p1).getByRole('button', { name: /Custom.*title-b/ })).toBeTruthy()
    const p2 = toggleOf('p2').closest('li')!
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
  })

  it('shows the missing badge and disables ＋ for a vanished root, but it can still be removed', async () => {
    projects.list.mockResolvedValue([project('gone', { missing: true })])
    renderTree()
    await screen.findByText('missing')
    expect(screen.getByRole('button', { name: 'New session in name-gone' })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: 'Remove name-gone' })).toHaveProperty('disabled', false)
  })

  it('a rejected project list shows the note instead of rows', async () => {
    projects.list.mockRejectedValue({ code: 'storage', message: 'cannot open goko.db', hint: null })
    renderTree()
    expect((await screen.findByText('cannot open goko.db')).closest('[role="alert"]')).toBeTruthy()
    expect(screen.queryByRole('list')).toBeNull()
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
      { agent: 'claude', projectId: 'p1', cols: 80, rows: 24 },
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
        { command: 'htop -d 5', projectId: 'p1', cols: 80, rows: 24 },
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
