import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, render, screen, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient, queryKeys } from '../../queryClient'
import type { AccountView, AccountsView } from '../../services/accounts'
import type { Subagent, SubagentMap } from '../../services/subagents'
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
const subagentsService = vi.hoisted(() => ({ get: vi.fn() }))
vi.mock('../../services/subagents', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../services/subagents')>()),
  subagentsService,
}))
const accounts = vi.hoisted(() => ({
  get: vi.fn(),
  add: vi.fn(),
  rename: vi.fn(),
  remove: vi.fn(),
  setActive: vi.fn(),
}))
vi.mock('../../services/accounts', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../services/accounts')>()),
  accountsService: accounts,
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
  accounts: {},
})

const claudeDefault: AccountView = { id: 'default', name: 'Default', dir: '/h/.claude', is_default: true }
const claudeWork: AccountView = { id: 'a1', name: 'Work', dir: '/h/.claude-work', is_default: false }
const codexDefault: AccountView = { id: 'default', name: 'Default', dir: '/h/.codex', is_default: true }

const twoClaudeView = (): AccountsView => ({
  claude: { active: 'a1', items: [claudeDefault, claudeWork] },
  codex: { active: 'default', items: [codexDefault] },
})

const oneClaudeView = (): AccountsView => ({
  claude: { active: 'default', items: [claudeDefault] },
  codex: { active: 'default', items: [codexDefault] },
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
  subagentsService.get.mockReset().mockResolvedValue({})
  accounts.get.mockReset().mockResolvedValue(oneClaudeView())
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

describe('SessionItems account label', () => {
  const ENV_ARGV = ['/usr/bin/env', 'CLAUDE_CONFIG_DIR=/h/.claude-work', 'claude', '--session-id', 'u1']

  const renderAccountRow = (argv: string[], agent: string | undefined, view: AccountsView) => {
    usageService.get.mockResolvedValue(snapshot())
    qc.setQueryData(queryKeys.accounts, view)
    return render(
      <SessionItems
        sessions={[session(argv)]}
        labelOf={() => undefined}
        agentOf={() => agent}
        selectedId={null}
        workspaceId="workspace"
        onSelect={() => {}}
      />,
      { wrapper },
    )
  }

  it('shows the account derived from an env-prefixed argv when the agent has two accounts', async () => {
    renderAccountRow(ENV_ARGV, 'claude', twoClaudeView())

    expect(await screen.findByText('· Work')).toBeTruthy()
  })

  it('keeps the session account after the active account changes', async () => {
    renderAccountRow(ENV_ARGV, 'claude', twoClaudeView())
    expect(await screen.findByText('· Work')).toBeTruthy()

    const switched = twoClaudeView()
    switched.claude.active = 'default'
    act(() => void qc.setQueryData(queryKeys.accounts, switched))

    expect(screen.getByText('· Work')).toBeTruthy()
  })

  it('shows no account label with a single account', async () => {
    renderAccountRow(ENV_ARGV, 'claude', oneClaudeView())
    await screen.findByRole('button')
    await waitFor(() => expect(qc.getQueryData(queryKeys.accounts)).toEqual(oneClaudeView()))

    expect(screen.queryByText('· Work')).toBeNull()
    expect(screen.queryByText('· Default')).toBeNull()
  })

  it('shows no account label for a non-agent session', async () => {
    renderAccountRow(ENV_ARGV, 'custom', twoClaudeView())
    await screen.findByRole('button')
    await waitFor(() => expect(qc.getQueryData(queryKeys.accounts)).toEqual(twoClaudeView()))

    expect(screen.queryByText('· Work')).toBeNull()
    expect(screen.queryByText('· Default')).toBeNull()
  })
})

describe('SessionItems sub-agents', () => {
  const subagent = (agent: string, pid: number, command = `${agent} run`): Subagent => ({ agent, pid, command })

  const renderSubagentRow = (map: SubagentMap, state: SessionInfo['state'] = 'running') => {
    subagentsService.get.mockResolvedValue(map)
    return render(
      <SessionItems
        sessions={[{ ...session(['claude']), state }]}
        labelOf={() => undefined}
        selectedId={null}
        workspaceId="workspace"
        onSelect={() => {}}
      />,
      { wrapper },
    )
  }

  it('shows ↳ devin for one devin sub-agent', async () => {
    renderSubagentRow({ 'daemon-session': [subagent('devin', 123, 'devin --model swe-2')] })

    const visible = await screen.findByText('↳ devin')
    expect(visible.getAttribute('aria-hidden')).toBe('true')
  })

  it('shows ↳ devin ×2 for two devin sub-agents', async () => {
    renderSubagentRow({ 'daemon-session': [subagent('devin', 1), subagent('devin', 2)] })

    expect(await screen.findByText('↳ devin ×2')).toBeTruthy()
  })

  it('lists different agents alphabetically with counts', async () => {
    renderSubagentRow({
      'daemon-session': [subagent('devin', 1), subagent('codex', 2), subagent('devin', 3)],
    })

    expect(await screen.findByText('↳ codex, devin ×2')).toBeTruthy()
  })

  it('puts pid and command in the title and an accessible name', async () => {
    renderSubagentRow({ 'daemon-session': [subagent('devin', 123, 'devin --model swe-2')] })

    const indicator = await screen.findByTitle('devin (pid 123): devin --model swe-2')
    expect(indicator.querySelector('.sr-only')?.textContent).toBe(
      'Running sub-agents: devin (pid 123): devin --model swe-2',
    )
  })

  it('shows no indicator for a running session with no map entry', async () => {
    renderSubagentRow({ 'other-session': [subagent('devin', 1)] })

    await waitFor(() => expect(qc.getQueryData(queryKeys.subagents)).toBeDefined())
    expect(screen.getByText('Claude')).toBeTruthy()
    expect(screen.queryByText(/↳/)).toBeNull()
    expect(screen.queryByText(/Running sub-agents:/)).toBeNull()
  })

  it('shows no indicator when the session is not running, even with a map entry', async () => {
    renderSubagentRow({ 'daemon-session': [subagent('devin', 1)] }, 'exited')

    await waitFor(() => expect(qc.getQueryData(queryKeys.subagents)).toBeDefined())
    expect(screen.getByText('Claude')).toBeTruthy()
    expect(screen.getByTitle('exited')).toBeTruthy()
    expect(screen.queryByText(/↳/)).toBeNull()
    expect(screen.queryByText(/Running sub-agents:/)).toBeNull()
  })

  it('shows working on the dot when a sub-agent runs after the session turn ended', async () => {
    activityService.get.mockResolvedValue({ 'daemon-session': 'done' })
    renderSubagentRow({ 'daemon-session': [subagent('devin', 1)] })

    expect(await screen.findByTitle('running · working')).toBeTruthy()
  })

  it('shows working on the dot when a sub-agent runs with no activity entry', async () => {
    renderSubagentRow({ 'daemon-session': [subagent('devin', 1)] })

    expect(await screen.findByTitle('running · working')).toBeTruthy()
  })

  it('keeps waiting on the dot when a sub-agent runs while the session waits', async () => {
    activityService.get.mockResolvedValue({ 'daemon-session': 'waiting' })
    renderSubagentRow({ 'daemon-session': [subagent('devin', 1)] })

    expect(await screen.findByTitle('running · waiting')).toBeTruthy()
  })

  it('keeps done on the dot when no sub-agent runs', async () => {
    activityService.get.mockResolvedValue({ 'daemon-session': 'done' })
    renderSubagentRow({})

    expect(await screen.findByTitle('running · done')).toBeTruthy()
  })
})
