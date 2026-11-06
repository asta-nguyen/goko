import type { UsageLimit } from '../../services/usage'

export type UsageTone = 'muted' | 'warning' | 'danger'

export const usageToneClass = {
  muted: 'text-muted',
  warning: 'text-warning',
  danger: 'text-danger',
} as const

export function usageTone(percentage: number): UsageTone {
  return percentage >= 90 ? 'danger' : percentage >= 80 ? 'warning' : 'muted'
}

export function usageValue(limit: UsageLimit, now = Date.now()): { text: string; tone: UsageTone } {
  if (limit.resets_at * 1000 <= now) return { text: 'reset', tone: 'muted' }
  return { text: `${Math.round(limit.used_pct)}%`, tone: usageTone(limit.used_pct) }
}

export function windowLabel(windowMinutes: number, provider: 'claude' | 'codex'): string {
  if (windowMinutes === 300) return '5h'
  if (windowMinutes === 10_080) return provider === 'codex' ? 'wk' : '7d'
  if (windowMinutes % 1440 === 0) return `${windowMinutes / 1440}d`
  if (windowMinutes % 60 === 0) return `${windowMinutes / 60}h`
  return `${windowMinutes}m`
}

export function resetTime(epochSeconds: number): string {
  return new Date(epochSeconds * 1000).toLocaleString()
}

export function usageAge(updatedAt: number | null, now = Date.now()): string {
  if (updatedAt === null) return 'unknown'
  const seconds = Math.max(0, Math.floor((now - updatedAt) / 1000))
  if (seconds < 60) return `${seconds}s ago`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`
  if (seconds < 86_400) return `${Math.floor(seconds / 3600)}h ago`
  return `${Math.floor(seconds / 86_400)}d ago`
}
