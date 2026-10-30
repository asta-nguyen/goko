import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient, queryKeys, upsertSession } from '../../queryClient'
import type { SessionInfo } from '../../services/types'
import { useCreateSession, useDaemonStatus, useSession, useSessionList, useStopSession } from './queries'

const { service } = vi.hoisted(() => ({
  service: { list: vi.fn(), create: vi.fn(), stop: vi.fn(), status: vi.fn() },
}))
vi.mock('../../services/session', () => ({ sessionService: service }))

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
const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>

beforeEach(() => {
  qc = makeQueryClient()
  Object.values(service).forEach((f) => f.mockReset())
})

describe('query client', () => {
  it('has the three defaults', () => {
    expect(qc.getDefaultOptions().queries).toMatchObject({ staleTime: Infinity, refetchOnWindowFocus: false, retry: false })
  })
})

describe('useSessionList / useSession', () => {
  it('loads the list in the daemon order', async () => {
    service.list.mockResolvedValue([session('a'), session('b')])
    const { result } = renderHook(() => useSessionList(), { wrapper })
    await waitFor(() => expect(result.current.data).toBeDefined())
    expect(result.current.data?.map((s) => s.id)).toEqual(['a', 'b'])
  })

  it('exposes a BridgeError when the list fails', async () => {
    service.list.mockRejectedValue({ code: 'daemon_unavailable', message: 'down', hint: null })
    const { result } = renderHook(() => useSessionList(), { wrapper })
    await waitFor(() => expect(result.current.error).not.toBeNull())
    expect(result.current.error).toMatchObject({ code: 'daemon_unavailable' })
  })

  it('keeps the same object for session a when session b changes', async () => {
    service.list.mockResolvedValue([session('a'), session('b')])
    const { result } = renderHook(() => ({ a: useSession('a'), b: useSession('b') }), { wrapper })
    await waitFor(() => expect(result.current.a.data).toBeDefined())
    const before = result.current.a.data

    await act(() => upsertSession(qc, session('b', { state: 'exited', exit_code: 0 })))
    await waitFor(() => expect(result.current.b.data?.state).toBe('exited'))
    expect(result.current.a.data).toBe(before)

    await act(() => upsertSession(qc, session('a', { state: 'stopping' })))
    await waitFor(() => expect(result.current.a.data?.state).toBe('stopping'))
  })

  it('returns undefined for no id or an unknown id', async () => {
    service.list.mockResolvedValue([session('a')])
    const { result } = renderHook(() => [useSession(null), useSession('zzz')] as const, { wrapper })
    await waitFor(() => expect(result.current[0].isSuccess).toBe(true))
    expect(result.current[0].data).toBeUndefined()
    expect(result.current[1].data).toBeUndefined()
  })
})

describe('useDaemonStatus', () => {
  it('loads the status', async () => {
    const status = { connected: true, default_shell: '/bin/zsh', home: '/h' }
    service.status.mockResolvedValue(status)
    const { result } = renderHook(() => useDaemonStatus(), { wrapper })
    await waitFor(() => expect(result.current.data).toEqual(status))
  })
})

describe('useCreateSession', () => {
  const input = { command: 'sh', cwd: '/tmp', cols: 80, rows: 24 }

  it('is pending while in flight and inserts the session on success', async () => {
    service.list.mockResolvedValue([session('a')])
    let resolve!: (s: SessionInfo) => void
    service.create.mockReturnValue(new Promise<SessionInfo>((r) => (resolve = r)))
    const { result } = renderHook(() => ({ list: useSessionList(), create: useCreateSession() }), { wrapper })
    await waitFor(() => expect(result.current.list.data).toBeDefined())

    act(() => result.current.create.mutate(input))
    await waitFor(() => expect(result.current.create.isPending).toBe(true))

    await act(async () => resolve(session('b')))
    await waitFor(() => expect(result.current.create.isSuccess).toBe(true))
    expect(result.current.list.data?.map((s) => s.id)).toEqual(['a', 'b'])
  })

  it('exposes a rejected create as a BridgeError', async () => {
    service.create.mockRejectedValue({ code: 'invalid_argument', message: 'bad quote', hint: null })
    const { result } = renderHook(() => useCreateSession(), { wrapper })
    act(() => result.current.mutate(input))
    await waitFor(() => expect(result.current.isError).toBe(true))
    expect(result.current.error).toMatchObject({ code: 'invalid_argument', message: 'bad quote' })
  })
})

describe('useStopSession', () => {
  it('calls stop with id and force and leaves the cache to events', async () => {
    service.stop.mockResolvedValue(undefined)
    const { result } = renderHook(() => useStopSession(), { wrapper })
    act(() => result.current.mutate({ id: 'a', force: true }))
    await waitFor(() => expect(result.current.isSuccess).toBe(true))
    expect(service.stop).toHaveBeenCalledWith('a', true)
    expect(qc.getQueryData(queryKeys.sessions)).toBeUndefined()
  })
})

describe('upsertSession', () => {
  it('refetches instead of caching a lone session when the list was never loaded', async () => {
    service.list.mockResolvedValue([session('a'), session('b')])
    const { result } = renderHook(() => useSessionList(), { wrapper })
    // event lands while the very first fetch is still in flight
    await act(() => upsertSession(qc, session('b')))
    await waitFor(() => expect(result.current.data?.map((s) => s.id)).toEqual(['a', 'b']))
  })
})
