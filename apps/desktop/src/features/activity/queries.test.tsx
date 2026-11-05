import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient, queryKeys } from '../../queryClient'
import type { ActivityMap } from '../../services/activity'
import type { SessionInfo } from '../../services/types'
import { useActivity, useSessionActivity } from './queries'

const service = vi.hoisted(() => ({ get: vi.fn() }))
vi.mock('../../services/activity', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../services/activity')>()),
  activityService: service,
}))

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

let qc: QueryClient
const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>

beforeEach(() => {
  qc = makeQueryClient()
  service.get.mockReset()
})

describe('activity queries', () => {
  it('loads the map through activity_get', async () => {
    const value: ActivityMap = { s1: 'working', s2: 'waiting' }
    service.get.mockResolvedValue(value)

    const { result } = renderHook(() => useActivity(), { wrapper })

    await waitFor(() => expect(result.current.data).toEqual(value))
    expect(service.get).toHaveBeenCalledOnce()
    expect(qc.getQueryData(queryKeys.activity)).toEqual(value)
  })

  it('returns the map entry only for a running session', async () => {
    service.get.mockResolvedValue({ s1: 'waiting', s2: 'done' })
    const { result } = renderHook(
      () => ({
        running: useSessionActivity(session('s1')),
        exited: useSessionActivity(session('s2', 'exited')),
        missing: useSessionActivity(session('unknown')),
      }),
      { wrapper },
    )

    await waitFor(() => expect(result.current.running).toBe('waiting'))
    expect(result.current.exited).toBeNull()
    expect(result.current.missing).toBeNull()
  })
})
