import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen } from '@testing-library/react'
import type { ReactNode } from 'react'
import { TooltipProvider } from '../../components/ui/tooltip'
import { makeQueryClient, queryKeys } from '../../queryClient'
import type { AccountView, AccountsView } from '../../services/accounts'
import type { AgentInfo } from '../../services/types'
import { NewSessionMenu } from './SessionLaunch'

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

const AGENTS: AgentInfo[] = [
  { id: 'claude', label: 'Claude Code', available: true, path: '/bin/claude' },
  { id: 'codex', label: 'Codex', available: true, path: '/bin/codex' },
  { id: 'devin', label: 'Devin', available: true, path: '/bin/devin' },
]

const account = (id: string, name: string, dir: string, isDefault = false): AccountView => ({
  id,
  name,
  dir,
  is_default: isDefault,
})

const claudeDefault = account('default', 'Default', '/h/.claude', true)
const claudeWork = account('a1', 'Work', '/h/.claude-work')
const codexDefault = account('default', 'Default', '/h/.codex', true)
const codexPersonal = account('c1', 'Personal', '/h/.codex-personal')

const view = (claude: AccountsView['claude'], codex: AccountsView['codex']): AccountsView => ({
  claude,
  codex,
})

const oneEach = () =>
  view(
    { active: 'default', items: [claudeDefault] },
    { active: 'default', items: [codexDefault] },
  )

const twoClaude = () =>
  view(
    { active: 'a1', items: [claudeDefault, claudeWork] },
    { active: 'default', items: [codexDefault] },
  )

const twoCodex = () =>
  view(
    { active: 'default', items: [claudeDefault] },
    { active: 'c1', items: [codexDefault, codexPersonal] },
  )

let qc: QueryClient
const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={qc}>
    <TooltipProvider>{children}</TooltipProvider>
  </QueryClientProvider>
)

const renderMenu = () =>
  render(
    <NewSessionMenu label="New session" agents={AGENTS} disabled={false} onAgent={() => {}} onCustom={() => {}} />,
    { wrapper },
  )

const openMenu = async () => {
  fireEvent.keyDown(screen.getByRole('button', { name: 'New session' }), { key: 'ArrowDown' })
}

beforeEach(() => {
  qc = makeQueryClient()
  accounts.get.mockReset().mockResolvedValue(oneEach())
})

describe('NewSessionMenu account hints', () => {
  it('appends the active account to the Claude item only when it has two accounts', async () => {
    qc.setQueryData(queryKeys.accounts, twoClaude())
    renderMenu()
    await openMenu()

    expect(await screen.findByRole('menuitem', { name: 'Claude Code · Work' })).toBeTruthy()
    expect(screen.getByRole('menuitem', { name: 'Codex' })).toBeTruthy()
    expect(screen.getByRole('menuitem', { name: 'Devin' })).toBeTruthy()
  })

  it('shows no account hint with a single account per agent', async () => {
    qc.setQueryData(queryKeys.accounts, oneEach())
    renderMenu()
    await openMenu()

    expect(await screen.findByRole('menuitem', { name: 'Claude Code' })).toBeTruthy()
    expect(screen.getByRole('menuitem', { name: 'Codex' })).toBeTruthy()
    expect(screen.getByRole('menuitem', { name: 'Devin' })).toBeTruthy()
  })

  it('shows the Codex hint independently of Claude', async () => {
    qc.setQueryData(queryKeys.accounts, twoCodex())
    renderMenu()
    await openMenu()

    expect(await screen.findByRole('menuitem', { name: 'Codex · Personal' })).toBeTruthy()
    expect(screen.getByRole('menuitem', { name: 'Claude Code' })).toBeTruthy()
  })
})
