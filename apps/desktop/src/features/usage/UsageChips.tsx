import { useEffect, useState } from 'react'
import { Badge } from '../../components/ui/badge'
import { Tooltip, TooltipContent, TooltipTrigger } from '../../components/ui/tooltip'
import { useUsage } from './queries'
import type { UsageLimit } from '../../services/usage'
import { resetTime, usageAge, usageToneClass, usageValue, windowLabel } from './tone'

function limitsFor(
  values: Array<UsageLimit | null>,
  provider: 'claude' | 'codex',
): Array<{ label: string; limit: UsageLimit }> {
  return values.flatMap((limit) => (limit ? [{ label: windowLabel(limit.window_minutes, provider), limit }] : []))
}

export function UsageChips() {
  const [now, setNow] = useState(() => Date.now())
  const { data } = useUsage()

  const hasLimits = Boolean(
    data && (data.claude.five_hour || data.claude.seven_day || data.codex.primary || data.codex.secondary),
  )
  useEffect(() => {
    if (!hasLimits) return
    setNow(Date.now())
    const timer = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(timer)
  }, [hasLimits])

  if (!data) return null

  const providers = [
    {
      key: 'claude',
      name: 'Claude',
      updatedAt: data.claude.updated_at,
      limits: limitsFor([data.claude.five_hour, data.claude.seven_day], 'claude'),
    },
    {
      key: 'codex',
      name: 'Codex',
      updatedAt: data.codex.updated_at,
      limits: limitsFor([data.codex.primary, data.codex.secondary], 'codex'),
    },
  ].filter((provider) => provider.limits.length > 0)

  if (providers.length === 0) return null

  return (
    <div className="flex items-center gap-2">
      {providers.map(({ key, name, updatedAt, limits }) => {
        const label = `${name} ${limits.map(({ label, limit }) => `${label} ${usageValue(limit, now).text}`).join(' · ')}`
        return (
          <Tooltip key={key}>
            <TooltipTrigger asChild>
              <Badge variant="outline" tabIndex={0} aria-label={label} className="cursor-help gap-1.5 px-2">
                <span>{name}</span>
                {limits.map(({ label: window, limit }, index) => {
                  const value = usageValue(limit, now)
                  return (
                    <span key={window} className="flex items-center gap-1">
                      {index > 0 && <span aria-hidden className="text-muted">·</span>}
                      <span className="text-muted">{window}</span>
                      <span className={`tabular-nums ${usageToneClass[value.tone]}`}>{value.text}</span>
                    </span>
                  )
                })}
              </Badge>
            </TooltipTrigger>
            <TooltipContent side="bottom" className="flex-col items-start gap-0.5">
              {limits.map(({ label: window, limit }) => (
                <div key={window}>{window} resets {resetTime(limit.resets_at)}</div>
              ))}
              <div>Updated {usageAge(updatedAt, now)}</div>
            </TooltipContent>
          </Tooltip>
        )
      })}
    </div>
  )
}
