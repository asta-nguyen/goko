import { accountsService, accountNameOf, type AccountsView } from './accounts'

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke }))

beforeEach(() => invoke.mockReset().mockResolvedValue(undefined))

const view: AccountsView = {
  claude: {
    active: 'default',
    items: [
      { id: 'default', name: 'Default', dir: '/home/u/.claude', is_default: true },
      { id: 'a1', name: 'Work', dir: '/home/u/.claude-work', is_default: false },
    ],
  },
  codex: {
    active: 'default',
    items: [
      { id: 'default', name: 'Default', dir: '/home/u/.codex', is_default: true },
      { id: 'c1', name: 'Personal', dir: '/home/u/.codex-personal', is_default: false },
    ],
  },
}

describe('accountNameOf', () => {
  it('returns null without a view', () => {
    expect(accountNameOf('claude', ['/usr/bin/env', 'CLAUDE_CONFIG_DIR=/x', 'claude'], undefined)).toBeNull()
  })

  it('returns the Default name for a plain argv', () => {
    expect(accountNameOf('claude', ['claude', '--session-id', 'u1'], view)).toBe('Default')
    expect(accountNameOf('codex', ['codex'], view)).toBe('Default')
  })

  it('returns the Default name for an env argv without an account env word', () => {
    expect(accountNameOf('claude', ['/usr/bin/env', 'claude'], view)).toBe('Default')
    // Env-looking words only count after an `env` first element.
    expect(accountNameOf('claude', ['claude', 'CLAUDE_CONFIG_DIR=/home/u/.claude-work'], view)).toBe('Default')
  })

  it('returns the matching account name for an env-prefixed argv', () => {
    expect(
      accountNameOf('claude', ['/usr/bin/env', 'CLAUDE_CONFIG_DIR=/home/u/.claude-work', '/usr/bin/claude'], view),
    ).toBe('Work')
    // A dir equal to the Default folder resolves to the Default row.
    expect(accountNameOf('claude', ['/usr/bin/env', 'CLAUDE_CONFIG_DIR=/home/u/.claude', 'claude'], view)).toBe(
      'Default',
    )
  })

  it('returns the folder’s last path component for an unmatched env word', () => {
    expect(accountNameOf('claude', ['/usr/bin/env', 'CLAUDE_CONFIG_DIR=/home/u/.claude-gone', 'claude'], view)).toBe(
      '.claude-gone',
    )
    expect(
      accountNameOf('claude', ['/usr/bin/env', 'CLAUDE_CONFIG_DIR=/home/u/.claude-gone/', 'claude'], view),
    ).toBe('.claude-gone')
  })

  it('reads CODEX_HOME for Codex and ignores the Claude word', () => {
    expect(accountNameOf('codex', ['/usr/bin/env', 'CODEX_HOME=/home/u/.codex-personal', 'codex'], view)).toBe(
      'Personal',
    )
    expect(accountNameOf('codex', ['/usr/bin/env', 'CLAUDE_CONFIG_DIR=/home/u/.claude-work', 'codex'], view)).toBe(
      'Default',
    )
  })
})

describe('accountsService.signIn', () => {
  it('calls accounts_sign_in with the agent and id', async () => {
    const launch = { command: 'env CLAUDE_CONFIG_DIR=/x claude auth login', cwd: '/x', title: 'Sign in · Work' }
    invoke.mockResolvedValueOnce(launch)

    await expect(accountsService.signIn('claude', 'a1')).resolves.toEqual(launch)
    expect(invoke).toHaveBeenCalledWith('accounts_sign_in', { agent: 'claude', id: 'a1' })
  })
})
