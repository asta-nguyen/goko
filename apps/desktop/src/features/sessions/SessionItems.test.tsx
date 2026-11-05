import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient, queryKeys } from '../../queryClient'
import type { UsageSnapshot } from '../../services/usage'
import type { SessionInfo } from '../../services/types'
import { SessionItems } from './SessionItems'

const usageService = vi.hoisted(() => ({ get: vi.fn() }))
vi.mock('../../services/usage', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../services/usage')>()),
  usageService,
}))
const activityService = vi.hoisted(() => ({ get: vi.fn() }))
vi.mock('../../services/activity', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../services/activity')>()),
  activityService,
}))

const session = (argv: string[]): SessionInfo => ({
  id: 'daemon-session',
  title: 'Claude',
  argv,
  cwd: '/tmp',
  state: 'running',
  exit_code: null,
  error: null,
  started_at: 1,
  ended_at: null,
})

const snapshot = (sessions: UsageSnapshot['claude']['sessions'] = {}): UsageSnapshot => ({
  claude: { five_hour: null, seven_day: null, sessions, updated_at: 1 },
  codex: { primary: null, secondary: null, updated_at: null },
})

let qc: QueryClient
const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>

function renderRows(value: UsageSnapshot, argv: string[]) {
  usageService.get.mockResolvedValue(value)
  return render(
    <SessionItems
      sessions={[session(argv)]}
      labelOf={() => undefined}
      selectedId={null}
      workspaceId="workspace"
      onSelect={() => {}}
    />,
    { wrapper },
  )
}

beforeEach(() => {
  qc = makeQueryClient()
  usageService.get.mockReset()
  activityService.get.mockReset().mockResolvedValue({})
})

describe('SessionItems usage context', () => {
  it('shows a Claude session context percentage at the row end', async () => {
    renderRows(snapshot({ u1: { context_pct: 62 } }), ['claude', '--session-id', 'u1'])

    const percentage = await screen.findByLabelText('Claude context 62%')
    expect(percentage.classList.contains('ml-auto')).toBe(true)
    expect(percentage.classList.contains('tabular-nums')).toBe(true)
    expect(percentage.classList.contains('text-muted')).toBe(true)
  })

  it('hides the context percentage when the session id has no value', async () => {
    const value = snapshot()
    renderRows(value, ['claude', '--session-id', 'missing'])

    await waitFor(() => expect(qc.getQueryData(queryKeys.usage)).toEqual(value))
    expect(screen.queryByLabelText(/Claude context/)).toBeNull()
    expect(screen.queryByText(/%$/)).toBeNull()
  })
})

describe('SessionItems activity', () => {
  it('shows the activity on the state dot for a running session', async () => {
    activityService.get.mockResolvedValue({ 'daemon-session': 'working' })
    renderRows(snapshot(), ['claude'])

    expect(await screen.findByTitle('running · working')).toBeDefined()
  })

  it('keeps the plain state title when the session is not running', async () => {
    activityService.get.mockResolvedValue({ 'daemon-session': 'working' })
    usageService.get.mockResolvedValue(snapshot())
    render(
      <SessionItems
        sessions={[{ ...session(['claude']), state: 'exited' }]}
        labelOf={() => undefined}
        selectedId={null}
        workspaceId="workspace"
        onSelect={() => {}}
      />,
      { wrapper },
    )

    expect(await screen.findByTitle('exited')).toBeDefined()
    expect(screen.queryByTitle('exited · working')).toBeNull()
  })
})
