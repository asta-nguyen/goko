import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, render, screen } from '@testing-library/react'
import { makeQueryClient, queryKeys } from '../../queryClient'
import { checkoutKey, useUi } from '../../stores/ui'
import type { SessionInfo } from '../../services/types'
import type { TerminalLinks } from './TerminalController'
import { TerminalView } from './TerminalView'

const { log, controllers, openUrlMock, fileList } = vi.hoisted(() => ({
  log: [] as string[],
  controllers: {} as Record<string, TerminalLinks | undefined>,
  openUrlMock: vi.fn(),
  fileList: vi.fn(),
}))
vi.mock('./TerminalController', () => ({
  TerminalController: class {
    constructor(private id: string, _service: unknown, links?: TerminalLinks) {
      log.push(`new:${id}`)
      controllers[id] = links
    }
    async mount(el: HTMLElement) {
      log.push(`mount:${this.id}:${el.tagName}`)
    }
    setVisible() {}
    dispose() {
      log.push(`dispose:${this.id}`)
    }
  },
}))
vi.mock('../../services/session', () => ({ sessionService: { list: vi.fn() } }))
vi.mock('../../services/git-view', () => ({ gitViewService: { fileList } }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: openUrlMock }))

const session = (id: string, over: Partial<SessionInfo> = {}): SessionInfo => ({
  id,
  title: id,
  argv: ['sh'],
  cwd: '/tmp',
  state: 'running',
  exit_code: null,
  error: null,
  started_at: 1,
  ended_at: null,
  ...over,
})

let qc: QueryClient
const ui = (id: string | null, props: Partial<Parameters<typeof TerminalView>[0]> = {}) => (
  <QueryClientProvider client={qc}>
    <TerminalView sessionId={id} {...props} />
  </QueryClientProvider>
)
const seed = (...s: SessionInfo[]) => qc.setQueryData(queryKeys.sessions, s)

beforeEach(() => {
  qc = makeQueryClient()
  log.length = 0
  openUrlMock.mockReset()
  fileList.mockReset()
  useUi.getState().reset()
})

describe('TerminalView', () => {
  it('shows the empty state and creates no controller without a session', () => {
    render(ui(null))
    expect(screen.getByText(/select or create a session/i)).toBeTruthy()
    expect(log).toEqual([])
  })

  it('mounts a controller on the host element and shows no banner for a running session', () => {
    seed(session('a'))
    render(ui('a'))
    expect(log).toEqual(['new:a', 'mount:a:DIV'])
    expect(screen.queryByRole('status')).toBeNull()
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('disposes the old controller before the new one mounts when the id changes', () => {
    seed(session('a'), session('b'))
    const { rerender } = render(ui('a'))
    rerender(ui('b'))
    expect(log).toEqual(['new:a', 'mount:a:DIV', 'dispose:a', 'new:b', 'mount:b:DIV'])
  })

  it('disposes on unmount', () => {
    seed(session('a'))
    const { unmount } = render(ui('a'))
    unmount()
    expect(log.at(-1)).toBe('dispose:a')
  })

  it('shows the exit code of an exited session and keeps the terminal', () => {
    seed(session('a', { state: 'exited', exit_code: 3 }))
    render(ui('a'))
    expect(screen.getByRole('status').textContent).toContain('code 3')
    expect(log).toContain('mount:a:DIV')
  })

  it('shows the error of a failed session', () => {
    seed(session('a', { state: 'failed', error: 'cwd does not exist' }))
    render(ui('a'))
    expect(screen.getByRole('alert').textContent).toContain('cwd does not exist')
  })
})

describe('TerminalView links', () => {
  const WS = checkoutKey('p1', '/work/p1')

  it('provides openUrl but no file links without project context', () => {
    seed(session('a'))
    render(ui('a', { workspaceId: WS }))
    const links = controllers['a']
    expect(typeof links?.openUrl).toBe('function')
    expect(links?.file).toBeUndefined()
  })

  it('omits file links when the workspace id is unknown', () => {
    seed(session('a'))
    render(ui('a', { projectId: 'p1', checkoutPath: '/work/p1' }))
    expect(controllers['a']?.file).toBeUndefined()
  })

  it('resolves printed paths against the session cwd through the shared file list', async () => {
    seed(session('a', { cwd: '/work/p1/src' }))
    fileList.mockResolvedValue(['src/a.ts', 'README.md'])
    render(ui('a', { workspaceId: WS, projectId: 'p1', checkoutPath: '/work/p1' }))
    const file = controllers['a']?.file
    expect(file).toBeDefined()

    await expect(file!.resolve('a.ts')).resolves.toBe('src/a.ts')
    await expect(file!.resolve('README.md')).resolves.toBe('README.md')
    expect(fileList).toHaveBeenCalledTimes(1)
    expect(fileList).toHaveBeenCalledWith('p1', '/work/p1')
  })

  it('keeps the mounted controller when project context arrives after mount', async () => {
    seed(session('a', { cwd: '/work/p1' }))
    fileList.mockResolvedValue(['src/a.ts'])
    const { rerender } = render(ui('a', { workspaceId: WS }))
    expect(controllers['a']?.file).toBeUndefined()

    rerender(ui('a', { workspaceId: WS, projectId: 'p1', checkoutPath: '/work/p1' }))
    expect(log).toEqual(['new:a', 'mount:a:DIV'])

    const file = controllers['a']?.file
    expect(file).toBeDefined()
    await expect(file!.resolve('src/a.ts')).resolves.toBe('src/a.ts')
    expect(fileList).toHaveBeenCalledWith('p1', '/work/p1')
  })

  it('resolves to null when the file list fails', async () => {
    seed(session('a'))
    fileList.mockRejectedValue(new Error('not a git repository'))
    render(ui('a', { workspaceId: WS, projectId: 'p1', checkoutPath: '/work/p1' }))
    await expect(controllers['a']!.file!.resolve('src/a.ts')).resolves.toBeNull()
  })

  it('opens a file through the Editor open request', () => {
    seed(session('a'))
    render(ui('a', { workspaceId: WS, projectId: 'p1', checkoutPath: '/work/p1' }))
    act(() => controllers['a']!.file!.open('src/a.ts', 7))
    expect(useUi.getState().editorOpenRequest).toMatchObject({ workspaceId: WS, path: 'src/a.ts', line: 7 })
  })

  it('handles a rejected opener promise', async () => {
    seed(session('a'))
    openUrlMock.mockRejectedValue(new Error('no browser'))
    render(ui('a'))
    controllers['a']!.openUrl('https://example.com')
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(openUrlMock).toHaveBeenCalledWith('https://example.com')
  })
})
