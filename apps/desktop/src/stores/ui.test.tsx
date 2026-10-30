import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient, queryKeys } from '../queryClient'
import type { SessionInfo } from '../services/types'
import { useSelectedId, useUi } from './ui'

vi.mock('../services/session', () => ({ sessionService: { list: vi.fn() } }))

const session = (id: string): SessionInfo => ({
  id,
  title: id,
  argv: ['sh'],
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
  qc.setQueryData(queryKeys.sessions, [session('a'), session('b')])
  useUi.setState({ selectedId: null })
})

describe('useSelectedId', () => {
  it('returns the selected id while that session exists', async () => {
    const { result } = renderHook(() => useSelectedId(), { wrapper })
    expect(result.current).toBeNull()
    act(() => useUi.getState().select('a'))
    expect(result.current).toBe('a')
  })

  it('returns null once the list loses the session, without calling select', async () => {
    const { result } = renderHook(() => useSelectedId(), { wrapper })
    act(() => useUi.getState().select('a'))
    expect(result.current).toBe('a')

    act(() => void qc.setQueryData(queryKeys.sessions, [session('b')]))
    await waitFor(() => expect(result.current).toBeNull())
    expect(useUi.getState().selectedId).toBe('a')
  })

  it('returns null for an unknown id', () => {
    const { result } = renderHook(() => useSelectedId(), { wrapper })
    act(() => useUi.getState().select('nope'))
    expect(result.current).toBeNull()
  })
})

describe('useUi', () => {
  it('holds only selection state, no session data', () => {
    expect(Object.keys(useUi.getState()).sort()).toEqual(['select', 'selectedId'])
  })
})
