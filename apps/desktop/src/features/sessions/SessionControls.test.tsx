import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { makeQueryClient, queryKeys } from '../../queryClient'
import type { DaemonStatus, SessionInfo } from '../../services/types'
import { SessionControls } from './SessionControls'

const { service } = vi.hoisted(() => ({ service: { list: vi.fn(), status: vi.fn(), stop: vi.fn() } }))
vi.mock('../../services/session', () => ({ sessionService: service }))

const session = (over: Partial<SessionInfo> = {}): SessionInfo => ({
  id: 'a',
  title: 'zsh',
  argv: ['/bin/zsh'],
  cwd: '/tmp',
  state: 'running',
  exit_code: null,
  error: null,
  started_at: Date.now(),
  ended_at: null,
  ...over,
})

let qc: QueryClient
const tick = (ms: number) => act(() => void vi.advanceTimersByTime(ms))
// the query cache notifies observers from a 0 ms timer, which fake timers hold back
const setSession = (s: SessionInfo) => {
  act(() => void qc.setQueryData(queryKeys.sessions, [s]))
  tick(0)
}
const setStatus = (connected: boolean) => {
  act(() => void qc.setQueryData<DaemonStatus>(queryKeys.daemonStatus, { connected, default_shell: '/bin/zsh', home: '/h' }))
  tick(0)
}
const renderControls = () =>
  render(
    <QueryClientProvider client={qc}>
      <SessionControls sessionId="a" />
    </QueryClientProvider>,
  )
const stopButton = () => screen.getByRole('button', { name: 'Stop' }) as HTMLButtonElement

beforeEach(() => {
  vi.useFakeTimers()
  qc = makeQueryClient()
  Object.values(service).forEach((f) => f.mockReset())
  service.stop.mockResolvedValue(undefined)
  qc.setQueryData(queryKeys.sessions, [session()])
  qc.setQueryData<DaemonStatus>(queryKeys.daemonStatus, { connected: true, default_shell: '/bin/zsh', home: '/h' })
})
afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('SessionControls', () => {
  it('shows the badge and a ticking elapsed time while running', () => {
    renderControls()
    expect(screen.getByText('running')).toBeTruthy()
    expect(screen.getByText('00:00')).toBeTruthy()
    tick(65_000)
    expect(screen.getByText('01:05')).toBeTruthy()
  })

  it('Stop calls the service with force:false', async () => {
    renderControls()
    await act(async () => void fireEvent.click(stopButton()))
    expect(service.stop).toHaveBeenCalledWith('a', false)
  })

  it('offers Force kill only after 5 s of stopping, and it forces', async () => {
    renderControls()
    expect(screen.queryByRole('button', { name: 'Force kill' })).toBeNull()
    setSession(session({ state: 'stopping' }))
    tick(4000)
    expect(screen.queryByRole('button', { name: 'Force kill' })).toBeNull()
    tick(1000)

    await act(async () => void fireEvent.click(screen.getByRole('button', { name: 'Force kill' })))
    expect(service.stop).toHaveBeenCalledWith('a', true)
  })

  it('hides Force kill again once the session exits', () => {
    renderControls()
    setSession(session({ state: 'stopping' }))
    tick(5000)
    expect(screen.getByRole('button', { name: 'Force kill' })).toBeTruthy()
    setSession(session({ state: 'exited', exit_code: 0, ended_at: Date.now() + 61_000 }))
    tick(0)
    expect(screen.queryByRole('button', { name: 'Force kill' })).toBeNull()
  })

  it.each(['exited', 'failed'] as const)('disables Stop for a %s session and freezes the elapsed time', (state) => {
    setSession(session({ state, ended_at: Date.now() + 61_000 }))
    renderControls()
    expect(stopButton().disabled).toBe(true)
    expect(screen.getByText('01:01')).toBeTruthy()
    tick(10_000)
    expect(screen.getByText('01:01')).toBeTruthy()
  })

  it('disables the controls while the daemon is disconnected, even with Force kill showing', () => {
    renderControls()
    setSession(session({ state: 'stopping' }))
    tick(5000)
    setStatus(false)
    expect(stopButton().disabled).toBe(true)
    expect((screen.getByRole('button', { name: 'Force kill' }) as HTMLButtonElement).disabled).toBe(true)
    setStatus(true)
    expect(stopButton().disabled).toBe(false)
  })

  it('shows a failed stop', async () => {
    service.stop.mockRejectedValue({ code: 'disconnected', message: 'daemon went away', hint: null })
    renderControls()
    await act(async () => void fireEvent.click(stopButton()))
    tick(0)
    expect(screen.getByRole('alert').textContent).toContain('daemon went away')
  })

  it('clears every interval on unmount', () => {
    const set = vi.spyOn(globalThis, 'setInterval')
    const clear = vi.spyOn(globalThis, 'clearInterval')
    const { unmount } = renderControls()
    expect(set).toHaveBeenCalled()
    unmount()
    for (const { value } of set.mock.results) expect(clear).toHaveBeenCalledWith(value)
  })

  it('does not tick for an exited session', () => {
    setSession(session({ state: 'exited', exit_code: 0, ended_at: Date.now() }))
    const set = vi.spyOn(globalThis, 'setInterval')
    renderControls()
    expect(set).not.toHaveBeenCalled()
  })
})
