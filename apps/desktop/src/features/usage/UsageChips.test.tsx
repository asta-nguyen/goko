import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { TooltipProvider } from '../../components/ui/tooltip'
import { makeQueryClient, queryKeys } from '../../queryClient'
import type { AccountView, AccountsView } from '../../services/accounts'
import type { UsageSnapshot } from '../../services/usage'
import { useUi } from '../../stores/ui'
import { UsageChips } from './UsageChips'

const usageService = vi.hoisted(() => ({ get: vi.fn() }))
vi.mock('../../services/usage', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../services/usage')>()),
  usageService,
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

const emptySnapshot = (): UsageSnapshot => ({
  claude: { five_hour: null, seven_day: null, sessions: {}, updated_at: null },
  codex: { primary: null, secondary: null, updated_at: null },
  accounts: {},
})

const claudeDefault: AccountView = { id: 'default', name: 'Default', dir: '/h/.claude', is_default: true }
const claudeWork: AccountView = { id: 'a1', name: 'Work', dir: '/h/.claude-work', is_default: false }
const codexDefault: AccountView = { id: 'default', name: 'Default', dir: '/h/.codex', is_default: true }

const singleAccounts = (): AccountsView => ({
  claude: { active: 'default', items: [claudeDefault] },
  codex: { active: 'default', items: [codexDefault] },
})

const twoClaudeAccounts = (active = 'a1'): AccountsView => ({
  claude: { active, items: [claudeDefault, claudeWork] },
  codex: { active: 'default', items: [codexDefault] },
})

let qc: QueryClient
const renderChips = () =>
  render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <UsageChips />
      </TooltipProvider>
    </QueryClientProvider>,
  )

beforeEach(() => {
  qc = makeQueryClient()
  usageService.get.mockReset()
  accounts.get.mockReset().mockResolvedValue(singleAccounts())
  accounts.setActive.mockReset().mockResolvedValue(twoClaudeAccounts('default'))
  useUi.getState().reset()
})

afterEach(() => vi.useRealTimers())

describe('UsageChips', () => {
  it('shows both providers, window labels, reset times, and figure age', async () => {
    const later = Math.floor(Date.now() / 1000) + 3600
    const value: UsageSnapshot = {
      claude: {
        five_hour: { used_pct: 42, resets_at: later, window_minutes: 300 },
        seven_day: { used_pct: 13, resets_at: later, window_minutes: 10_080 },
        sessions: {},
        updated_at: Date.now() - 60_000,
      },
      codex: {
        primary: { used_pct: 56, resets_at: later, window_minutes: 300 },
        secondary: { used_pct: 13, resets_at: later, window_minutes: 10_080 },
        updated_at: Date.now(),
      },
      accounts: {},
    }
    usageService.get.mockResolvedValue(value)
    renderChips()

    const claude = await screen.findByLabelText('Claude 5h 42% · 7d 13%')
    expect(screen.getByLabelText('Codex 5h 56% · wk 13%')).toBeTruthy()
    fireEvent.focus(claude)
    expect(await screen.findByText(/5h resets/)).toBeTruthy()
    expect(await screen.findByText('Updated 1m ago')).toBeTruthy()
  })

  it('renders an expired limit as muted reset text', async () => {
    const value = emptySnapshot()
    value.claude.five_hour = { used_pct: 42, resets_at: 1, window_minutes: 300 }
    value.claude.updated_at = Date.now()
    usageService.get.mockResolvedValue(value)
    renderChips()

    expect(await screen.findByLabelText('Claude 5h reset')).toBeTruthy()
    expect(screen.getByText('reset').classList.contains('text-muted')).toBe(true)
    expect(screen.queryByText('42%')).toBeNull()
  })

  it('updates reset and age labels while the app is idle', () => {
    vi.useFakeTimers()
    const now = 1_700_000_000_000
    vi.setSystemTime(now)
    const value = emptySnapshot()
    value.claude.five_hour = { used_pct: 42, resets_at: now / 1000 + 1, window_minutes: 300 }
    value.claude.updated_at = now
    qc.setQueryData(queryKeys.usage, value)
    renderChips()

    expect(screen.getByLabelText('Claude 5h 42%')).toBeTruthy()
    act(() => vi.advanceTimersByTime(1000))

    expect(screen.getByLabelText('Claude 5h reset')).toBeTruthy()
    fireEvent.focus(screen.getByLabelText('Claude 5h reset'))
    expect(screen.getByText('Updated 1s ago')).toBeTruthy()
  })

  it.each([
    ['session context only', { sessions: { u1: { context_pct: 62 } } }],
    ['no usage data', {}],
  ])('renders no header chip with %s', async (_label, claude) => {
    const value = emptySnapshot()
    Object.assign(value.claude, claude)
    usageService.get.mockResolvedValue(value)
    renderChips()

    await waitFor(() => expect(qc.getQueryData(queryKeys.usage)).toEqual(value))
    expect(screen.queryByLabelText(/Claude/)).toBeNull()
    expect(screen.queryByLabelText(/Codex/)).toBeNull()
  })
})

describe('UsageChips with multiple accounts', () => {
  const seedTwoClaudeAccounts = async () => {
    const later = Math.floor(Date.now() / 1000) + 3600
    const value = emptySnapshot()
    value.claude.five_hour = { used_pct: 42, resets_at: later, window_minutes: 300 }
    value.claude.seven_day = { used_pct: 13, resets_at: later, window_minutes: 10_080 }
    value.claude.updated_at = Date.now() - 60_000
    value.accounts = {
      'claude:a1': {
        limits: [
          { used_pct: 42, resets_at: later, window_minutes: 300 },
          { used_pct: 10, resets_at: later, window_minutes: 10_080 },
        ],
        updated_at: Date.now() - 3 * 3_600_000,
      },
    }
    usageService.get.mockResolvedValue(value)
    qc.setQueryData(queryKeys.accounts, twoClaudeAccounts())
    renderChips()
    return screen.findByLabelText('Claude · Work 5h 42% · 7d 13%')
  }

  it('names the active account on the chip and opens a menu listing every account', async () => {
    const chip = await seedTwoClaudeAccounts()
    fireEvent.keyDown(chip, { key: 'ArrowDown' })

    const work = await screen.findByRole('menuitemradio', { name: /Work/ })
    const fallback = screen.getByRole('menuitemradio', { name: /Default/ })
    expect(work.getAttribute('aria-checked')).toBe('true')
    expect(fallback.getAttribute('aria-checked')).toBe('false')
    expect(work.textContent).toContain('5h 42%')
    expect(fallback.textContent).toContain('no usage yet')
    expect(screen.getByRole('menuitem', { name: 'Manage accounts…' })).toBeTruthy()
  })

  it('does nothing when the already-active account row is picked', async () => {
    const chip = await seedTwoClaudeAccounts()
    fireEvent.keyDown(chip, { key: 'ArrowDown' })
    fireEvent.click(await screen.findByRole('menuitemradio', { name: /Work/ }))
    await act(async () => {})

    expect(accounts.setActive).not.toHaveBeenCalled()
  })

  it('calls accounts_set_active when another account is picked', async () => {
    const chip = await seedTwoClaudeAccounts()
    fireEvent.keyDown(chip, { key: 'ArrowDown' })
    fireEvent.click(await screen.findByRole('menuitemradio', { name: /Default/ }))

    await waitFor(() => expect(accounts.setActive).toHaveBeenCalledWith('claude', 'default'))
  })

  it('shows the chip without limits while two accounts exist', async () => {
    usageService.get.mockResolvedValue(emptySnapshot())
    qc.setQueryData(queryKeys.accounts, twoClaudeAccounts())
    renderChips()

    expect(await screen.findByLabelText('Claude · Work')).toBeTruthy()
    expect(screen.queryByLabelText(/Codex/)).toBeNull()
  })

  it('opens Settings › Accounts from Manage accounts…', async () => {
    const chip = await seedTwoClaudeAccounts()
    fireEvent.keyDown(chip, { key: 'ArrowDown' })
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Manage accounts…' }))

    await waitFor(() => {
      expect(useUi.getState().activeView).toBe('settings')
      expect(useUi.getState().settingsSection).toBe('accounts')
    })
  })
})
