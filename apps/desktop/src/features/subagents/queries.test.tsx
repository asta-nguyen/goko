import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient, queryKeys } from '../../queryClient'
import type { Subagent, SubagentMap } from '../../services/subagents'
import type { SessionInfo } from '../../services/types'
import { useSessionSubagents, useSubagents } from './queries'

const service = vi.hoisted(() => ({ get: vi.fn() }))
vi.mock('../../services/subagents', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../services/subagents')>()),
  subagentsService: service,
}))

const subagent = (agent: string, pid: number): Subagent => ({ agent, pid, command: `${agent} --pid ${pid}` })

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

describe('subagent queries', () => {
  it('loads the map through subagents_get', async () => {
    const value: SubagentMap = { s1: [subagent('devin', 1)], s2: [subagent('codex', 2)] }
    service.get.mockResolvedValue(value)

    const { result } = renderHook(() => useSubagents(), { wrapper })

    await waitFor(() => expect(result.current.data).toEqual(value))
    expect(service.get).toHaveBeenCalledOnce()
    expect(qc.getQueryData(queryKeys.subagents)).toEqual(value)
  })

  it('returns the session entry only while it is running', async () => {
    service.get.mockResolvedValue({ s1: [subagent('devin', 1)], s2: [subagent('codex', 2)] })
    const { result } = renderHook(
      () => ({
        running: useSessionSubagents(session('s1')),
        exited: useSessionSubagents(session('s2', 'exited')),
        missing: useSessionSubagents(session('unknown')),
      }),
      { wrapper },
    )

    await waitFor(() => expect(result.current.running).toEqual([subagent('devin', 1)]))
    expect(result.current.exited).toEqual([])
    expect(result.current.missing).toEqual([])
    expect(result.current.exited).toBe(result.current.missing)
  })

  it('follows cache replacements like subagents://updated', async () => {
    const first: SubagentMap = { s1: [subagent('devin', 1)] }
    const second: SubagentMap = { s1: [subagent('devin', 1), subagent('codex', 2)] }
    service.get.mockResolvedValue(first)
    const { result } = renderHook(() => useSessionSubagents(session('s1')), { wrapper })

    await waitFor(() => expect(result.current).toEqual(first.s1))

    act(() => void qc.setQueryData(queryKeys.subagents, second))

    await waitFor(() => expect(result.current).toEqual(second.s1))
  })
})
