import { call } from './call'

export interface UsageLimit {
  used_pct: number
  /** Unix epoch seconds. */
  resets_at: number
  window_minutes: number
}

/** One account's usage: its limits in window order (empty when it has no data). */
export interface AccountUsage {
  limits: UsageLimit[]
  /** Unix epoch milliseconds. */
  updated_at: number | null
}

export interface UsageSnapshot {
  claude: {
    five_hour: UsageLimit | null
    seven_day: UsageLimit | null
    sessions: Record<string, { context_pct: number }>
    /** Unix epoch milliseconds. */
    updated_at: number | null
  }
  codex: {
    primary: UsageLimit | null
    secondary: UsageLimit | null
    /** Unix epoch milliseconds. */
    updated_at: number | null
  }
  /** Every account's usage, keyed `"<agent>:<account-id>"` (e.g. `claude:default`). */
  accounts: Record<string, AccountUsage>
}

export const usageService = {
  get: () => call<UsageSnapshot>('usage_get'),
}

export function claudeSessionId(argv: string[]): string | null {
  const flag = argv.indexOf('--session-id')
  return flag === -1 ? null : (argv[flag + 1] ?? null)
}
