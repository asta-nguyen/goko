import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen } from '@testing-library/react'
import { makeQueryClient, queryKeys } from '../../queryClient'
import type { SessionInfo } from '../../services/types'
import { TerminalView } from './TerminalView'

const { log } = vi.hoisted(() => ({ log: [] as string[] }))
vi.mock('./TerminalController', () => ({
  TerminalController: class {
    constructor(private id: string) {
      log.push(`new:${id}`)
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
const ui = (id: string | null) => (
  <QueryClientProvider client={qc}>
    <TerminalView sessionId={id} />
  </QueryClientProvider>
)
const seed = (...s: SessionInfo[]) => qc.setQueryData(queryKeys.sessions, s)

beforeEach(() => {
  qc = makeQueryClient()
  log.length = 0
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
