import { call } from './call'

// Hand-mirrored from src-tauri `accounts.rs` (the `accounts_*` commands).
export type AccountAgent = 'claude' | 'codex'

export interface AccountView {
  id: string
  name: string
  dir: string
  is_default: boolean
}

export interface AgentAccountsView {
  active: string
  /** Default first. */
  items: AccountView[]
}

export interface AccountsView {
  claude: AgentAccountsView
  codex: AgentAccountsView
}

/** The `accounts_sign_in` payload: the agent's login command for
 * `session_create`'s custom-command path, the account folder as `cwd`, and
 * the session title. */
export interface SignInLaunch {
  command: string
  cwd: string
  title: string
}

export const accountsService = {
  get: () => call<AccountsView>('accounts_get'),
  add: (agent: AccountAgent, name: string, dir: string) =>
    call<AccountsView>('accounts_add', { agent, name, dir }),
  rename: (agent: AccountAgent, id: string, name: string) =>
    call<AccountsView>('accounts_rename', { agent, id, name }),
  remove: (agent: AccountAgent, id: string) => call<AccountsView>('accounts_remove', { agent, id }),
  setActive: (agent: AccountAgent, id: string) =>
    call<AccountsView>('accounts_set_active', { agent, id }),
  signIn: (agent: AccountAgent, id: string) => call<SignInLaunch>('accounts_sign_in', { agent, id }),
}

export type AccountsService = typeof accountsService

const ENV_PREFIX: Record<AccountAgent, string> = {
  claude: 'CLAUDE_CONFIG_DIR=',
  codex: 'CODEX_HOME=',
}

/** Last path component of `dir`, ignoring a trailing slash. */
const lastComponent = (dir: string) => {
  const trimmed = dir.replace(/\/+$/, '')
  return trimmed.slice(trimmed.lastIndexOf('/') + 1) || dir
}

/**
 * The account a session argv runs under: the `CLAUDE_CONFIG_DIR=` or
 * `CODEX_HOME=` word after an `env` first element, matched to an account's
 * dir. No env word means Default; a word matching no account (a removed one)
 * is labelled by its folder's last path component.
 */
export function accountNameOf(
  agent: AccountAgent,
  argv: string[],
  view: AccountsView | undefined,
): string | null {
  if (view === undefined) return null
  const agentView = view[agent]
  const word =
    lastComponent(argv[0] ?? '') === 'env'
      ? argv.slice(1).find((arg) => arg.startsWith(ENV_PREFIX[agent]))
      : undefined
  if (word === undefined) {
    return agentView.items.find((item) => item.is_default)?.name ?? null
  }
  const dir = word.slice(ENV_PREFIX[agent].length)
  return agentView.items.find((item) => item.dir === dir)?.name ?? lastComponent(dir)
}
