import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, render, screen, waitFor } from '@testing-library/react'
import { makeQueryClient, queryKeys, upsertSession } from '../../queryClient'
import type { SessionInfo } from '../../services/types'
import { useUi } from '../../stores/ui'
import { SessionList } from './SessionList'

const { renders } = vi.hoisted(() => ({ renders: [] as string[] }))
vi.mock('../../services/session', () => ({ sessionService: { list: vi.fn() } }))
vi.mock('./StateBadge', () => ({
  StateBadge: ({ state }: { state: string }) => {
    renders.push(state)
    return <span>{state}</span>
  },
}))

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
beforeEach(() => {
  qc = makeQueryClient()
  renders.length = 0
  useUi.setState({ selectedId: null })
})

it('shows an empty hint without sessions', () => {
  qc.setQueryData(queryKeys.sessions, [])
  render(
    <QueryClientProvider client={qc}>
      <SessionList />
    </QueryClientProvider>,
  )
  expect(screen.getByText(/no sessions yet/i)).toBeTruthy()
})

it('re-renders only the row whose session changed', async () => {
  qc.setQueryData(queryKeys.sessions, [session('a'), session('b')])
  render(
    <QueryClientProvider client={qc}>
      <SessionList />
    </QueryClientProvider>,
  )
  expect(renders).toEqual(['running', 'running'])

  await act(() => upsertSession(qc, session('b', { state: 'exited' })))
  await waitFor(() => expect(screen.getByText('exited')).toBeTruthy())
  expect(renders).toEqual(['running', 'running', 'exited'])
})
