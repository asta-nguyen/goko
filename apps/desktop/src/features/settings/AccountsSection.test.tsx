import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient, queryKeys } from '../../queryClient'
import type { AccountView, AgentAccountsView, AccountsView } from '../../services/accounts'
import type { SessionInfo } from '../../services/types'
import type { UsageSnapshot } from '../../services/usage'
import { UNASSIGNED_WORKSPACE, useUi } from '../../stores/ui'
import { AccountsSection } from './AccountsSection'

const accounts = vi.hoisted(() => ({
  get: vi.fn(),
  add: vi.fn(),
  rename: vi.fn(),
  remove: vi.fn(),
  setActive: vi.fn(),
  signIn: vi.fn(),
}))
vi.mock('../../services/accounts', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../services/accounts')>()),
  accountsService: accounts,
}))

const session = vi.hoisted(() => ({
  create: vi.fn(),
  list: vi.fn(),
  meta: vi.fn(),
  status: vi.fn(),
  stop: vi.fn(),
}))
vi.mock('../../services/session', () => ({ sessionService: session }))

const usageService = vi.hoisted(() => ({ get: vi.fn() }))
vi.mock('../../services/usage', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../services/usage')>()),
  usageService,
}))

const claudeDefault: AccountView = { id: 'default', name: 'Default', dir: '/home/u/.claude', is_default: true }
const work: AccountView = { id: 'a1', name: 'Work', dir: '/home/u/.claude-work', is_default: false }
const codexDefault: AccountView = { id: 'default', name: 'Default', dir: '/home/u/.codex', is_default: true }

const view = (claude?: Partial<AgentAccountsView>, codex?: Partial<AgentAccountsView>): AccountsView => ({
  claude: { active: 'default', items: [claudeDefault, work], ...claude },
  codex: { active: 'default', items: [codexDefault], ...codex },
})

const snapshot = (accountsMap: UsageSnapshot['accounts'] = {}): UsageSnapshot => ({
  claude: { five_hour: null, seven_day: null, sessions: {}, updated_at: null },
  codex: { primary: null, secondary: null, updated_at: null },
  accounts: accountsMap,
})

const makeSession = (id: string, over: Partial<SessionInfo> = {}): SessionInfo => ({
  id,
  title: '',
  argv: ['env', 'CLAUDE_CONFIG_DIR=/x', 'claude', 'auth', 'login'],
  cwd: '/x',
  state: 'running',
  exit_code: null,
  error: null,
  started_at: 1,
  ended_at: null,
  ...over,
})

const signInLaunch = {
  command: 'env CLAUDE_CONFIG_DIR=/home/u/.claude-work claude auth login',
  cwd: '/home/u/.claude-work',
  title: 'Sign in · Work',
}

let qc: QueryClient
const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={qc}>{children}</QueryClientProvider>
)
const renderSection = () => render(<AccountsSection />, { wrapper })

beforeEach(() => {
  qc = makeQueryClient()
  accounts.get.mockReset().mockResolvedValue(view())
  accounts.add.mockReset().mockResolvedValue(view())
  accounts.rename.mockReset().mockResolvedValue(view())
  accounts.remove.mockReset().mockResolvedValue(view())
  accounts.setActive.mockReset().mockResolvedValue(view())
  accounts.signIn.mockReset().mockResolvedValue(signInLaunch)
  session.create.mockReset().mockResolvedValue(makeSession('s1'))
  session.list.mockReset().mockResolvedValue([])
  session.meta.mockReset().mockResolvedValue([])
  session.status.mockReset().mockResolvedValue({ connected: true, default_shell: '/bin/zsh', home: '/h' })
  session.stop.mockReset().mockResolvedValue(undefined)
  usageService.get.mockReset().mockResolvedValue(snapshot())
  qc.setQueryData(queryKeys.usage, snapshot())
  useUi.getState().reset()
})

describe('AccountsSection', () => {
  it('shows a read-only Default row with its folder on both cards', async () => {
    renderSection()

    const claude = await screen.findByRole('group', { name: 'Claude Code accounts' })
    const claudeDefaultRow = await within(claude).findByRole('group', { name: 'Default' })
    expect(claudeDefaultRow.textContent).toContain('/home/u/.claude')
    expect(within(claudeDefaultRow).getByText('Active')).toBeTruthy()
    expect(within(claudeDefaultRow).queryByRole('button', { name: /More actions/ })).toBeNull()
    expect(within(claudeDefaultRow).queryByRole('button', { name: 'Set active' })).toBeNull()

    const codex = await screen.findByRole('group', { name: 'Codex accounts' })
    const codexDefaultRow = await within(codex).findByRole('group', { name: 'Default' })
    expect(codexDefaultRow.textContent).toContain('/home/u/.codex')
    expect(within(codexDefaultRow).queryByRole('button', { name: /More actions/ })).toBeNull()
  })

  it('sends the trimmed name and folder to accounts_add and closes the form on success', async () => {
    const added = view({
      active: 'a2',
      items: [claudeDefault, work, { id: 'a2', name: 'Side project', dir: '~/.claude-side-project', is_default: false }],
    })
    accounts.add.mockResolvedValue(added)
    renderSection()
    const claude = await screen.findByRole('group', { name: 'Claude Code accounts' })

    fireEvent.click(within(claude).getByRole('button', { name: 'Add account' }))
    const nameInput = await within(claude).findByLabelText('Name')
    const folderInput = within(claude).getByLabelText('Folder') as HTMLInputElement
    expect(folderInput.value).toBe('~/.claude-')

    fireEvent.change(nameInput, { target: { value: '  Side project  ' } })
    expect(folderInput.value).toBe('~/.claude-side-project')
    fireEvent.click(within(claude).getByRole('button', { name: 'Add account' }))

    await waitFor(() =>
      expect(accounts.add).toHaveBeenCalledWith('claude', 'Side project', '~/.claude-side-project'),
    )
    await waitFor(() => expect(within(claude).queryByLabelText('Name')).toBeNull())
    expect(await within(claude).findByRole('group', { name: 'Side project' })).toBeTruthy()
  })

  it('shows a rejected add inline and keeps the form and typed text', async () => {
    accounts.add.mockRejectedValue({
      code: 'invalid_argument',
      message: 'an account named Work already exists',
      hint: null,
    })
    renderSection()
    const claude = await screen.findByRole('group', { name: 'Claude Code accounts' })

    fireEvent.click(within(claude).getByRole('button', { name: 'Add account' }))
    const nameInput = (await within(claude).findByLabelText('Name')) as HTMLInputElement
    const folderInput = within(claude).getByLabelText('Folder') as HTMLInputElement
    fireEvent.change(nameInput, { target: { value: 'Work' } })
    fireEvent.change(folderInput, { target: { value: '/data/work' } })
    fireEvent.click(within(claude).getByRole('button', { name: 'Add account' }))

    await waitFor(() => expect(accounts.add).toHaveBeenCalledWith('claude', 'Work', '/data/work'))
    const alert = await within(claude).findByRole('alert')
    expect(alert.textContent).toContain('an account named Work already exists')
    expect(nameInput.value).toBe('Work')
    expect(folderInput.value).toBe('/data/work')
  })

  it('tracks the name in the folder field until the folder is edited', async () => {
    renderSection()
    const claude = await screen.findByRole('group', { name: 'Claude Code accounts' })

    fireEvent.click(within(claude).getByRole('button', { name: 'Add account' }))
    const nameInput = (await within(claude).findByLabelText('Name')) as HTMLInputElement
    const folderInput = within(claude).getByLabelText('Folder') as HTMLInputElement

    fireEvent.change(nameInput, { target: { value: 'My Account' } })
    expect(folderInput.value).toBe('~/.claude-my-account')

    fireEvent.change(folderInput, { target: { value: '/data/custom' } })
    fireEvent.change(nameInput, { target: { value: 'Other Name' } })
    expect(folderInput.value).toBe('/data/custom')
  })

  it('sets an added account active through accounts_set_active', async () => {
    renderSection()
    const claude = await screen.findByRole('group', { name: 'Claude Code accounts' })
    const workRow = await within(claude).findByRole('group', { name: 'Work' })

    fireEvent.click(within(workRow).getByRole('button', { name: 'Set active' }))

    await waitFor(() => expect(accounts.setActive).toHaveBeenCalledWith('claude', 'a1'))
  })

  it('sets Default active from its read-only row', async () => {
    accounts.get.mockResolvedValue(view({ active: 'a1' }))
    renderSection()
    const claude = await screen.findByRole('group', { name: 'Claude Code accounts' })
    const defaultRow = await within(claude).findByRole('group', { name: 'Default' })

    fireEvent.click(within(defaultRow).getByRole('button', { name: 'Set active' }))

    await waitFor(() => expect(accounts.setActive).toHaveBeenCalledWith('claude', 'default'))
  })

  it('renames an account inline through accounts_rename', async () => {
    accounts.rename.mockResolvedValue(
      view({ items: [claudeDefault, { ...work, name: 'Dayjob' }] }),
    )
    renderSection()
    const claude = await screen.findByRole('group', { name: 'Claude Code accounts' })
    const workRow = await within(claude).findByRole('group', { name: 'Work' })

    fireEvent.keyDown(within(workRow).getByRole('button', { name: 'More actions for Work' }), { key: 'ArrowDown' })
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Rename' }))

    const input = (await within(workRow).findByLabelText('Rename Work')) as HTMLInputElement
    expect(input.value).toBe('Work')
    fireEvent.change(input, { target: { value: 'Dayjob' } })
    fireEvent.click(within(workRow).getByRole('button', { name: 'Save' }))

    await waitFor(() => expect(accounts.rename).toHaveBeenCalledWith('claude', 'a1', 'Dayjob'))
    expect(await within(claude).findByRole('group', { name: 'Dayjob' })).toBeTruthy()
  })

  it('removes an account through accounts_remove and keeps it out of the list', async () => {
    accounts.remove.mockResolvedValue(view({ items: [claudeDefault] }))
    renderSection()
    const claude = await screen.findByRole('group', { name: 'Claude Code accounts' })
    const workRow = await within(claude).findByRole('group', { name: 'Work' })

    fireEvent.keyDown(within(workRow).getByRole('button', { name: 'More actions for Work' }), { key: 'ArrowDown' })
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Remove' }))

    await waitFor(() => expect(accounts.remove).toHaveBeenCalledWith('claude', 'a1'))
    await waitFor(() => expect(within(claude).queryByRole('group', { name: 'Work' })).toBeNull())
  })

  it('shows limits and age from the usage accounts map and no usage yet without data', async () => {
    const later = Math.floor(Date.now() / 1000) + 3600
    qc.setQueryData(
      queryKeys.usage,
      snapshot({
        'claude:a1': {
          limits: [
            { used_pct: 42, resets_at: later, window_minutes: 300 },
            { used_pct: 10, resets_at: later, window_minutes: 10_080 },
          ],
          updated_at: Date.now() - 3 * 3_600_000,
        },
        'claude:default': { limits: [], updated_at: null },
      }),
    )
    renderSection()

    const claude = await screen.findByRole('group', { name: 'Claude Code accounts' })
    const workRow = await within(claude).findByRole('group', { name: 'Work' })
    expect(workRow.textContent).toContain('5h 42%')
    expect(workRow.textContent).toContain('7d 10%')
    expect(workRow.textContent).toContain('updated 3h ago')

    const claudeDefaultRow = within(claude).getByRole('group', { name: 'Default' })
    expect(within(claudeDefaultRow).getByText('no usage yet')).toBeTruthy()

    const codex = await screen.findByRole('group', { name: 'Codex accounts' })
    const codexDefaultRow = within(codex).getByRole('group', { name: 'Default' })
    expect(within(codexDefaultRow).getByText('no usage yet')).toBeTruthy()
  })

  it('shows the linked-entries hint on each card', async () => {
    renderSection()
    await screen.findByRole('group', { name: 'Claude Code accounts' })
    await screen.findByRole('group', { name: 'Codex accounts' })

    expect(screen.getAllByText(/never shared/)).toHaveLength(2)
  })

  it('starts sign-in for the new account after a successful add and opens the session', async () => {
    const added = view({
      active: 'a2',
      items: [
        claudeDefault,
        work,
        { id: 'a2', name: 'Side project', dir: '/home/u/.claude-side-project', is_default: false },
      ],
    })
    accounts.add.mockResolvedValue(added)
    accounts.signIn.mockResolvedValue({
      command: 'env CLAUDE_CONFIG_DIR=/home/u/.claude-side-project claude auth login',
      cwd: '/home/u/.claude-side-project',
      title: 'Sign in · Side project',
    })
    session.create.mockResolvedValue(makeSession('s-new'))
    renderSection()
    const claude = await screen.findByRole('group', { name: 'Claude Code accounts' })

    fireEvent.click(within(claude).getByRole('button', { name: 'Add account' }))
    fireEvent.change(await within(claude).findByLabelText('Name'), { target: { value: 'Side project' } })
    fireEvent.click(within(claude).getByRole('button', { name: 'Add account' }))

    await waitFor(() => expect(accounts.signIn).toHaveBeenCalledWith('claude', 'a2'))
    await waitFor(() =>
      expect(session.create).toHaveBeenCalledWith(
        {
          command: 'env CLAUDE_CONFIG_DIR=/home/u/.claude-side-project claude auth login',
          cwd: '/home/u/.claude-side-project',
          title: 'Sign in · Side project',
          cols: 80,
          rows: 24,
        },
        expect.anything(),
      ),
    )
    await waitFor(() => {
      const ui = useUi.getState()
      expect(ui.activeWorkspaceId).toBe(UNASSIGNED_WORKSPACE)
      expect(ui.selectedId).toBe('s-new')
      expect(ui.activeView).toBe('terminal')
    })
  })

  it('shows Sign in on added rows only and opens the sign-in session for that account', async () => {
    session.create.mockResolvedValue(makeSession('s-work'))
    renderSection()
    const claude = await screen.findByRole('group', { name: 'Claude Code accounts' })
    const codex = await screen.findByRole('group', { name: 'Codex accounts' })

    const workRow = await within(claude).findByRole('group', { name: 'Work' })
    const claudeDefaultRow = within(claude).getByRole('group', { name: 'Default' })
    expect(within(workRow).getByRole('button', { name: 'Sign in' })).toBeTruthy()
    expect(within(claudeDefaultRow).queryByRole('button', { name: 'Sign in' })).toBeNull()
    expect(within(codex).queryByRole('button', { name: 'Sign in' })).toBeNull()

    fireEvent.click(within(workRow).getByRole('button', { name: 'Sign in' }))

    await waitFor(() => expect(accounts.signIn).toHaveBeenCalledWith('claude', 'a1'))
    await waitFor(() =>
      expect(session.create).toHaveBeenCalledWith(
        {
          command: signInLaunch.command,
          cwd: signInLaunch.cwd,
          title: signInLaunch.title,
          cols: 80,
          rows: 24,
        },
        expect.anything(),
      ),
    )
    await waitFor(() => {
      const ui = useUi.getState()
      expect(ui.activeWorkspaceId).toBe(UNASSIGNED_WORKSPACE)
      expect(ui.selectedId).toBe('s-work')
      expect(ui.activeView).toBe('terminal')
    })
  })

  it('shows a rejected sign-in inline in the card and keeps the account listed', async () => {
    accounts.signIn.mockRejectedValue({
      code: 'invalid_argument',
      message: 'claude is not on PATH',
      hint: 'install it',
    })
    renderSection()
    const claude = await screen.findByRole('group', { name: 'Claude Code accounts' })
    const workRow = await within(claude).findByRole('group', { name: 'Work' })

    fireEvent.click(within(workRow).getByRole('button', { name: 'Sign in' }))

    const alert = await within(claude).findByRole('alert')
    expect(alert.textContent).toContain('claude is not on PATH')
    expect(session.create).not.toHaveBeenCalled()
    expect(await within(claude).findByRole('group', { name: 'Work' })).toBeTruthy()
  })
})
