import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { useDaemonStatus, useSessionList } from '../features/sessions/queries'
import { makeQueryClient, queryKeys } from '../queryClient'
import { startQuerySync } from './sync'
import type { DaemonStatus, SessionInfo } from './types'

type Handler = (e: { payload: unknown }) => void
const { handlers, listen, service } = vi.hoisted(() => {
  const handlers = new Map<string, Set<Handler>>()
  const listen = vi.fn(async (name: string, cb: Handler) => {
    if (!handlers.has(name)) handlers.set(name, new Set())
    handlers.get(name)!.add(cb)
    return () => void handlers.get(name)!.delete(cb)
  })
  return { handlers, listen, service: { list: vi.fn(), status: vi.fn() } }
})
vi.mock('@tauri-apps/api/event', () => ({ listen }))
vi.mock('./session', () => ({ sessionService: service }))

const emit = (name: string, payload: unknown) => handlers.get(name)?.forEach((cb) => cb({ payload }))
const listeners = () => [...handlers.values()].reduce((n, set) => n + set.size, 0)
const settle = () => new Promise((r) => setTimeout(r, 0))

const session = (id: string, state: SessionInfo['state'] = 'running'): SessionInfo => ({
  id,
  title: id,
  argv: ['sh'],
  cwd: '/tmp',
  state,
  exit_code: null,
  error: null,
  started_at: 1,
  ended_at: null,
})
const cached = () => qc.getQueryData<SessionInfo[]>(queryKeys.sessions)

let qc: QueryClient
let stop: () => void
beforeEach(async () => {
  qc = makeQueryClient()
  handlers.clear()
  listen.mockClear()
  Object.values(service).forEach((f) => f.mockReset())
  stop = startQuerySync(qc)
  await settle()
})
afterEach(() => stop())

describe('startQuerySync', () => {
  it('updates one session without replacing the others', async () => {
    const [a, b] = [session('a'), session('b')]
    qc.setQueryData(queryKeys.sessions, [a, b])
    emit('session://updated', session('b', 'stopping'))
    await waitFor(() => expect(cached()?.[1].state).toBe('stopping'))
    expect(cached()?.[0]).toBe(a)
  })

  it('treats session://exited the same way', async () => {
    qc.setQueryData(queryKeys.sessions, [session('a')])
    emit('session://exited', { ...session('a', 'exited'), exit_code: 3 })
    await waitFor(() => expect(cached()?.[0]).toMatchObject({ state: 'exited', exit_code: 3 }))
  })

  it('inserts an unknown id', async () => {
    qc.setQueryData(queryKeys.sessions, [session('a')])
    emit('session://updated', session('new'))
    await waitFor(() => expect(cached()?.map((s) => s.id)).toEqual(['a', 'new']))
  })

  it('is not overwritten by a fetch that was in flight when the event arrived', async () => {
    qc.setQueryData(queryKeys.sessions, [session('a')])
    let resolveFetch!: (l: SessionInfo[]) => void
    const fetching = qc
      .fetchQuery({
        queryKey: queryKeys.sessions,
        staleTime: 0,
        queryFn: () => new Promise<SessionInfo[]>((r) => (resolveFetch = r)),
      })
      .catch(() => {}) // cancelled by the event

    emit('session://updated', session('a', 'stopping'))
    await waitFor(() => expect(cached()?.[0].state).toBe('stopping'))
    resolveFetch([session('a', 'running')]) // stale result arrives late
    await fetching
    await settle()
    expect(cached()?.[0].state).toBe('stopping')
  })

  it('on disconnect marks the status disconnected, on reconnect refetches each key once', async () => {
    service.list.mockResolvedValue([session('a')])
    const status: DaemonStatus = { connected: true, default_shell: '/bin/zsh', home: '/h' }
    service.status.mockResolvedValue(status)
    const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>
    const { result } = renderHook(() => ({ list: useSessionList(), status: useDaemonStatus() }), { wrapper })
    await waitFor(() => expect(result.current.status.data).toBeDefined())
    await waitFor(() => expect(result.current.list.data).toBeDefined())

    emit('daemon://status', { connected: false })
    await waitFor(() => expect(result.current.status.data?.connected).toBe(false))
    await settle()
    expect([service.list.mock.calls.length, service.status.mock.calls.length]).toEqual([1, 1])

    emit('daemon://status', { connected: true })
    await waitFor(() => expect([service.list.mock.calls.length, service.status.mock.calls.length]).toEqual([2, 2]))
    await settle()
    expect([service.list.mock.calls.length, service.status.mock.calls.length]).toEqual([2, 2])
    expect(result.current.status.data?.connected).toBe(true)
  })

  it('refetches once its listeners are registered, in case the bridge connected earlier', async () => {
    const other = makeQueryClient()
    const disconnected: DaemonStatus = { connected: false, default_shell: '/bin/zsh', home: '/h' }
    service.status.mockResolvedValueOnce(disconnected).mockResolvedValue({ ...disconnected, connected: true })
    service.list.mockResolvedValue([])
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={other}>{children}</QueryClientProvider>
    )
    const { result } = renderHook(() => useDaemonStatus(), { wrapper })
    await waitFor(() => expect(result.current.data?.connected).toBe(false))

    const stopOther = startQuerySync(other) // the `connected` event already happened
    await waitFor(() => expect(result.current.data?.connected).toBe(true))
    stopOther()
  })

  it('registers three listeners once, and stop removes them all', async () => {
    expect(listeners()).toBe(3)
    expect(startQuerySync(qc)).toBe(stop)
    await settle()
    expect(listen).toHaveBeenCalledTimes(3)

    stop()
    await settle()
    expect(listeners()).toBe(0)
  })

  it('stop right after start still removes the listeners that register later', async () => {
    const other = makeQueryClient()
    const stopOther = startQuerySync(other)
    stopOther() // before `listen` resolved
    await settle()
    expect(listeners()).toBe(3) // only the beforeEach client's
  })
})
