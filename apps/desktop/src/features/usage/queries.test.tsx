import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient, queryKeys } from '../../queryClient'
import type { UsageSnapshot } from '../../services/usage'
import type { SessionInfo } from '../../services/types'
import { useSessionContext, useUsage } from './queries'

const service = vi.hoisted(() => ({ get: vi.fn() }))
vi.mock('../../services/usage', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../services/usage')>()),
  usageService: service,
}))

const snapshot = (sessions: UsageSnapshot['claude']['sessions'] = {}): UsageSnapshot => ({
  claude: { five_hour: null, seven_day: null, sessions, updated_at: 1 },
  codex: { primary: null, secondary: null, updated_at: null },
  accounts: {},
})

const session = (argv: string[]): SessionInfo => ({
  id: 'daemon-session',
  title: 'claude',
  argv,
  cwd: '/tmp',
  state: 'running',
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

describe('usage queries', () => {
  it('loads the snapshot through usage_get', async () => {
    const value = snapshot()
    service.get.mockResolvedValue(value)

    const { result } = renderHook(() => useUsage(), { wrapper })

    await waitFor(() => expect(result.current.data).toEqual(value))
    expect(service.get).toHaveBeenCalledOnce()
    expect(qc.getQueryData(queryKeys.usage)).toEqual(value)
  })

  it('returns Claude context only when the session id exists in the snapshot', async () => {
    const value = snapshot({ u1: { context_pct: 62 } })
    service.get.mockResolvedValue(value)
    const { result } = renderHook(
      () => ({ known: useSessionContext(session(['claude', '--session-id', 'u1'])), missing: useSessionContext(session(['claude'])) }),
      { wrapper },
    )

    await waitFor(() => expect(result.current.known).toBe(62))
    expect(result.current.missing).toBeNull()
  })
})
