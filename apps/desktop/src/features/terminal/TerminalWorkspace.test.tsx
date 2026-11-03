import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient, queryKeys } from '../../queryClient'
import type { Project, SessionInfo } from '../../services/types'
import { checkoutKey, useUi } from '../../stores/ui'
import { TerminalWorkspace } from './TerminalWorkspace'

const { sessionService, projectService, agentService, worktreeService } = vi.hoisted(() => ({
  sessionService: {
    list: vi.fn(), status: vi.fn(), meta: vi.fn(), create: vi.fn(), stop: vi.fn(), delete: vi.fn(),
  },
  projectService: { list: vi.fn() },
  agentService: { list: vi.fn() },
  worktreeService: { list: vi.fn() },
}))
vi.mock('../../services/session', () => ({ sessionService }))
vi.mock('../../services/projects', () => ({ projectService }))
vi.mock('../../services/agents', () => ({ agentService }))
vi.mock('../../services/worktrees', () => ({ worktreeService }))
vi.mock('./TerminalView', () => ({
  TerminalView: ({ sessionId, active }: { sessionId: string; active: boolean }) => (
    <div data-testid={`terminal-${sessionId}`} data-active={active} />
  ),
}))

const project: Project = {
  id: 'p1', name: 'Project One', root: '/work/one', created_at: 1, missing: false,
}
const workspaceId = checkoutKey(project.id, project.root)
const makeSession = (id: string, title = 'Shell', state: SessionInfo['state'] = 'running'): SessionInfo => ({
  id,
  title,
  argv: ['/bin/zsh'],
  cwd: project.root,
  state,
  exit_code: state === 'exited' ? 0 : null,
  error: null,
  started_at: 1,
  ended_at: state === 'exited' ? 2 : null,
})

let qc: QueryClient
let sessions: SessionInfo[]
let nextSession: number
let resizeCallbacks: (() => void)[]
const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={qc}>{children}</QueryClientProvider>
)
const view = (active = true) => render(
  <TerminalWorkspace workspaceId={workspaceId} active={active} project={project} checkoutPath={project.root} />,
  { wrapper },
)
const setSessions = (next: SessionInfo[]) => {
  sessions = next
  act(() => void qc.setQueryData(queryKeys.sessions, sessions))
}

beforeEach(() => {
  qc = makeQueryClient()
  sessions = []
  nextSession = 0
  resizeCallbacks = []
  useUi.getState().reset()
  Object.values(sessionService).forEach((mock) => mock.mockReset())
  sessionService.list.mockImplementation(async () => [...sessions])
  sessionService.status.mockResolvedValue({ connected: true, default_shell: '/bin/zsh', home: '/h' })
  sessionService.meta.mockResolvedValue([])
  sessionService.create.mockImplementation(async ({ title }: { title?: string }) => {
    const created = makeSession(`s${++nextSession}`, title || 'Shell')
    sessions = [...sessions, created]
    return created
  })
  sessionService.stop.mockImplementation(async (id: string) => {
    sessions = sessions.map((session) => session.id === id ? { ...session, state: 'stopping' } : session)
  })
  sessionService.delete.mockImplementation(async (id: string) => {
    sessions = sessions.filter((session) => session.id !== id)
  })
  projectService.list.mockResolvedValue([project])
  agentService.list.mockResolvedValue([])
  worktreeService.list.mockResolvedValue({ repo: false, worktrees: [] })
  vi.stubGlobal('ResizeObserver', class {
    constructor(callback: () => void) {
      resizeCallbacks.push(callback)
    }
    observe() {}
    disconnect() {}
  })
})

afterEach(() => vi.unstubAllGlobals())

describe('TerminalWorkspace', () => {
  it('keeps a missing linked checkout visible but disables every new terminal launch', async () => {
    const linkedPath = '/work/one-worktrees/feature'
    const linkedWorkspaceId = checkoutKey(project.id, linkedPath)
    act(() => {
      qc.setQueryData(queryKeys.worktrees(project.id), {
        repo: true,
        worktrees: [
          { name: 'one', path: project.root, branch: 'main', head: '12345678', is_root: true, missing: false, dirty: false, changed: 0 },
          { name: 'feature', path: linkedPath, branch: 'feature', head: 'abcdef12', is_root: false, missing: true, dirty: null, changed: null },
        ],
      })
      useUi.getState().activateWorkspace(linkedWorkspaceId)
    })
    render(
      <QueryClientProvider client={qc}>
        <TerminalWorkspace workspaceId={linkedWorkspaceId} active project={project} checkoutPath={linkedPath} />
      </QueryClientProvider>,
    )

    expect((await screen.findByRole('alert')).textContent).toMatch(/worktree is missing/i)
    expect(screen.getByRole('button', { name: 'New tab' })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: 'Split left/right' })).toHaveProperty('disabled', true)
    expect(sessionService.create).not.toHaveBeenCalled()
  })

  it('creates one default Shell tab even when the project is reactivated during launch', async () => {
    let resolve!: (session: SessionInfo) => void
    sessionService.create.mockReturnValueOnce(new Promise((done) => { resolve = done }))
    const { rerender } = view()
    await waitFor(() => expect(sessionService.create).toHaveBeenCalledTimes(1))

    rerender(
      <QueryClientProvider client={qc}>
        <TerminalWorkspace workspaceId={workspaceId} active={false} project={project} checkoutPath={project.root} />
      </QueryClientProvider>,
    )
    rerender(
      <QueryClientProvider client={qc}>
        <TerminalWorkspace workspaceId={workspaceId} active project={project} checkoutPath={project.root} />
      </QueryClientProvider>,
    )
    expect(sessionService.create).toHaveBeenCalledTimes(1)

    await act(async () => resolve(makeSession('default')))
    expect(await screen.findByRole('tab', { name: 'Shell' })).toBeTruthy()
  })

  it('keeps a failed default launch retryable without adding a phantom tab', async () => {
    sessionService.create
      .mockRejectedValueOnce(new Error('cannot start shell'))
      .mockImplementationOnce(async () => {
        const created = makeSession('retried')
        sessions = [...sessions, created]
        return created
      })
    view()

    expect((await screen.findByRole('alert')).textContent).toContain('cannot start shell')
    expect(screen.queryByRole('tab')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    expect(await screen.findByRole('tab', { name: 'Shell' })).toBeTruthy()
    expect(sessionService.create).toHaveBeenCalledTimes(2)
  })

  it('lets a single active session pane fill its tab panel', async () => {
    view()
    await screen.findByRole('tab', { name: 'Shell' })

    const panel = screen.getByRole('tabpanel', { name: 'Shell' })
    const pane = screen.getByTestId('terminal-s1').parentElement
    expect(panel.className).toContain('absolute inset-0 flex')
    expect(pane?.className).toContain('flex-1')
  })

  it('creates additional tabs and nested splits in the same project with keyboard resizing', async () => {
    view()
    await screen.findByRole('tab', { name: 'Shell' })

    fireEvent.click(screen.getByRole('button', { name: 'New tab' }))
    await screen.findByRole('tab', { name: 'Shell 2' })
    fireEvent.click(screen.getByRole('button', { name: 'Split left/right' }))
    const vertical = await screen.findByRole('separator')
    expect(vertical.getAttribute('aria-orientation')).toBe('vertical')
    const panel = screen.getByRole('tabpanel', { name: 'Shell 2' })
    expect((panel.firstElementChild as HTMLElement).className).toContain('flex-1')
    expect(screen.getByTestId('terminal-s2').getAttribute('data-active')).toBe('true')
    expect(screen.getByTestId('terminal-s3').getAttribute('data-active')).toBe('true')
    expect(sessionService.create).toHaveBeenLastCalledWith(
      { agent: 'shell', projectId: 'p1', checkout: project.root, cols: 80, rows: 24, title: 'Shell' },
      expect.anything(),
    )

    fireEvent.keyDown(vertical, { key: 'End' })
    expect(vertical.getAttribute('aria-valuenow')).toBe('95')
    fireEvent.click(screen.getByRole('button', { name: 'Split top/bottom' }))
    await waitFor(() => expect(sessionService.create).toHaveBeenCalledTimes(4))
    const separators = screen.getAllByRole('separator')
    expect(separators.map((separator) => separator.getAttribute('aria-orientation')).sort()).toEqual(['horizontal', 'vertical'])
    expect(sessionService.create).toHaveBeenCalledTimes(4)
  })

  it('supports pointer drag and clamps both sides to the pane minimum', async () => {
    view()
    await screen.findByRole('tab', { name: 'Shell' })
    fireEvent.click(screen.getByRole('button', { name: 'Split left/right' }))
    const separator = await screen.findByRole('separator')
    const container = separator.parentElement!
    vi.spyOn(container, 'getBoundingClientRect').mockReturnValue({
      x: 0, y: 0, left: 0, top: 0, right: 500, bottom: 400, width: 500, height: 400, toJSON: () => ({}),
    })
    act(() => resizeCallbacks[0]())
    expect(separator.getAttribute('aria-valuemin')).toBe('37')
    expect(separator.getAttribute('aria-valuemax')).toBe('63')

    fireEvent.pointerDown(separator, { pointerId: 1, clientX: 10, clientY: 100, buttons: 1 })
    await waitFor(() => expect(separator.getAttribute('aria-valuenow')).toBe('37'))
    fireEvent.pointerMove(separator, { pointerId: 1, clientX: 490, clientY: 100, buttons: 1 })
    await waitFor(() => expect(separator.getAttribute('aria-valuenow')).toBe('63'))
  })

  it('keeps a late split session visible when its target tab closes during creation', async () => {
    view()
    await screen.findByRole('tab', { name: 'Shell' })

    let resolve!: (session: SessionInfo) => void
    sessionService.create.mockReturnValueOnce(new Promise((done) => { resolve = done }))
    fireEvent.click(screen.getByRole('button', { name: 'Split left/right' }))
    await waitFor(() => expect(sessionService.create).toHaveBeenCalledTimes(2))

    fireEvent.click(screen.getByRole('button', { name: 'Close Shell tab' }))
    await waitFor(() => expect(sessionService.stop).toHaveBeenCalledWith('s1', true))
    setSessions([makeSession('s1', 'Shell', 'exited')])
    await waitFor(() => expect(screen.queryByRole('tab', { name: 'Shell' })).toBeNull())

    const late = makeSession('late-split')
    sessions = [...sessions, late]
    await act(async () => resolve(late))

    expect(await screen.findByRole('tab', { name: 'Shell' })).toBeTruthy()
    expect(screen.getByTestId('terminal-late-split')).toBeTruthy()
  })

  it('reserves enough width for nested splits so panes are not clipped', async () => {
    view()
    await screen.findByRole('tab', { name: 'Shell' })
    fireEvent.click(screen.getByRole('button', { name: 'Split left/right' }))
    await waitFor(() => expect(sessionService.create).toHaveBeenCalledTimes(2))
    fireEvent.click(screen.getByRole('button', { name: 'Split left/right' }))

    await waitFor(() => expect(sessionService.create).toHaveBeenCalledTimes(3))
    const separators = await screen.findAllByRole('separator')
    const root = separators[0].parentElement!
    const nested = separators[1].parentElement!
    const layoutBeforeResize = useUi.getState().workspaces[workspaceId].tabs[0].layout
    expect(root.className).toContain('flex-1')
    expect(screen.getAllByTestId(/^terminal-/).map((terminal) => terminal.getAttribute('data-active'))).toEqual(['true', 'true', 'true'])
    vi.spyOn(root, 'getBoundingClientRect').mockReturnValue({
      x: 0, y: 0, left: 0, top: 0, right: 556, bottom: 400, width: 556, height: 400, toJSON: () => ({}),
    })
    vi.spyOn(nested, 'getBoundingClientRect').mockReturnValue({
      x: 0, y: 0, left: 0, top: 0, right: 368, bottom: 400, width: 368, height: 400, toJSON: () => ({}),
    })
    act(() => resizeCallbacks.forEach((callback) => callback()))
    expect(root.style.minWidth).toBe('556px')
    expect((root.children[2] as HTMLElement).style.minWidth).toBe('368px')
    expect(nested.style.minWidth).toBe('368px')
    expect(separators[0].getAttribute('aria-valuenow')).toBe(separators[0].getAttribute('aria-valuemin'))
    expect(separators[0].getAttribute('aria-valuenow')).toBe(separators[0].getAttribute('aria-valuemax'))
    expect(useUi.getState().workspaces[workspaceId].tabs[0].layout).toEqual(layoutBeforeResize)
  })

  it('waits for exit before deleting a live session and then removes the closed tab', async () => {
    view()
    await screen.findByRole('tab', { name: 'Shell' })

    fireEvent.click(screen.getByRole('button', { name: 'Close Shell tab' }))
    await waitFor(() => expect(sessionService.stop).toHaveBeenCalledWith('s1', true))
    expect(sessionService.delete).not.toHaveBeenCalled()
    expect(screen.getByRole('tab', { name: /Shell.*Closing/ })).toBeTruthy()

    setSessions([makeSession('s1', 'Shell', 'exited')])
    await waitFor(() => expect(sessionService.delete).toHaveBeenCalledWith('s1'))
    await waitFor(() => expect(screen.queryByRole('tab', { name: 'Shell' })).toBeNull())
  })

  it('stops and deletes every live pane before removing a multi-pane tab', async () => {
    view()
    await screen.findByRole('tab', { name: 'Shell' })
    fireEvent.click(screen.getByRole('button', { name: 'Split left/right' }))
    await waitFor(() => expect(sessionService.create).toHaveBeenCalledTimes(2))
    expect(screen.getByRole('separator')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Close Shell tab' }))
    await waitFor(() => {
      expect(sessionService.stop).toHaveBeenCalledWith('s1', true)
      expect(sessionService.stop).toHaveBeenCalledWith('s2', true)
    })
    expect(sessionService.delete).not.toHaveBeenCalled()

    setSessions([makeSession('s1', 'Shell', 'exited'), makeSession('s2', 'Shell', 'exited')])
    await waitFor(() => {
      expect(sessionService.delete).toHaveBeenCalledWith('s1')
      expect(sessionService.delete).toHaveBeenCalledWith('s2')
    })
    await waitFor(() => expect(screen.queryByRole('tab', { name: 'Shell' })).toBeNull())
    expect(screen.queryByRole('separator')).toBeNull()
  })

  it('closes one pane and collapses the split after that session is deleted', async () => {
    view()
    await screen.findByRole('tab', { name: 'Shell' })
    fireEvent.click(screen.getByRole('button', { name: 'Split left/right' }))
    await waitFor(() => expect(sessionService.create).toHaveBeenCalledTimes(2))
    expect(screen.getByRole('separator')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Close pane' }))
    await waitFor(() => expect(sessionService.stop).toHaveBeenCalledWith('s2', true))
    setSessions([makeSession('s1'), makeSession('s2', 'Shell', 'exited')])
    await waitFor(() => expect(sessionService.delete).toHaveBeenCalledWith('s2'))

    expect(screen.queryByRole('separator')).toBeNull()
    expect(screen.getByRole('tab', { name: 'Shell' })).toBeTruthy()
    expect(screen.getByTestId('terminal-s1')).toBeTruthy()
    expect(screen.queryByTestId('terminal-s2')).toBeNull()
  })

  it('skips Stop for an exited session and keeps a failed delete visible for retry', async () => {
    const exited = makeSession('finished', 'Done', 'exited')
    sessions = [exited]
    qc.setQueryData(queryKeys.sessions, sessions)
    useUi.getState().activateWorkspace(workspaceId)
    useUi.getState().openSession(workspaceId, exited.id, exited.title)
    sessionService.delete.mockRejectedValueOnce(new Error('daemon does not support session.delete'))
    view()

    fireEvent.click(screen.getByRole('button', { name: 'Close Done tab' }))
    expect(sessionService.stop).not.toHaveBeenCalled()
    expect((await screen.findByRole('alert')).textContent).toContain('session.delete')
    expect(screen.getByRole('tab', { name: /Done.*Closing/ })).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    await waitFor(() => expect(sessionService.delete).toHaveBeenCalledTimes(2))
    await waitFor(() => expect(screen.queryByRole('tab', { name: 'Done' })).toBeNull())
  })

  it('force-stops a stopping pane immediately when its tab closes', async () => {
    view()
    await screen.findByRole('tab', { name: 'Shell' })
    setSessions([makeSession('s1', 'Shell', 'stopping')])

    fireEvent.click(screen.getByRole('button', { name: 'Close Shell tab' }))
    await waitFor(() => expect(sessionService.stop).toHaveBeenCalledWith('s1', true))
  })

  it('keeps a starting session visibly Closing until the daemon accepts stop', async () => {
    view()
    await screen.findByRole('tab', { name: 'Shell' })
    setSessions([makeSession('s1', 'Shell', 'starting')])

    fireEvent.click(screen.getByRole('button', { name: 'Close Shell tab' }))
    expect(await screen.findByRole('tab', { name: /Shell.*Closing/ })).toBeTruthy()
    expect(sessionService.stop).not.toHaveBeenCalled()
    expect(sessionService.delete).not.toHaveBeenCalled()

    setSessions([makeSession('s1', 'Shell', 'running')])
    await waitFor(() => expect(sessionService.stop).toHaveBeenCalledWith('s1', true))
  })

  it('deletes a created session that finishes naturally without stopping it', async () => {
    view()
    await screen.findByRole('tab', { name: 'Shell' })
    setSessions([makeSession('s1', 'Shell', 'created')])

    fireEvent.click(screen.getByRole('button', { name: 'Close Shell tab' }))
    expect(sessionService.stop).not.toHaveBeenCalled()

    setSessions([makeSession('s1', 'Shell', 'failed')])
    await waitFor(() => expect(sessionService.delete).toHaveBeenCalledWith('s1'))
    expect(sessionService.stop).not.toHaveBeenCalled()
    await waitFor(() => expect(screen.queryByRole('tab', { name: 'Shell' })).toBeNull())
  })

  it('keeps a failed force-stop visible and retries with force while still live', async () => {
    view()
    await screen.findByRole('tab', { name: 'Shell' })
    sessionService.stop.mockRejectedValueOnce(new Error('cannot signal session'))

    fireEvent.click(screen.getByRole('button', { name: 'Close Shell tab' }))
    expect((await screen.findByRole('alert')).textContent).toContain('cannot signal session')
    expect(screen.getByRole('tab', { name: /Shell.*Closing/ })).toBeTruthy()
    expect(sessionService.delete).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    await waitFor(() => expect(sessionService.stop).toHaveBeenCalledTimes(2))
    expect(sessionService.stop).toHaveBeenLastCalledWith('s1', true)
  })
})
