import { Fragment } from 'react'
import type { AccountAgent } from '../../services/accounts'
import type { AccountUsage } from '../../services/usage'
import { usageAge, usageToneClass, usageValue, windowLabel } from '../usage/tone'

/** One line of per-account limits, e.g. `5h 42% · 7d 10% · updated 3h ago`. */
export function AccountUsageSummary({
  agent,
  usage,
}: {
  agent: AccountAgent
  usage: AccountUsage | undefined
}) {
  if (usage === undefined || usage.limits.length === 0) {
    return <span className="text-xs text-muted">no usage yet</span>
  }
  return (
    <span className="text-xs text-muted">
      {usage.limits.map((limit, index) => {
        const value = usageValue(limit)
        return (
          <Fragment key={limit.window_minutes}>
            {index > 0 && ' · '}
            {windowLabel(limit.window_minutes, agent)}{' '}
            <span className={usageToneClass[value.tone]}>{value.text}</span>
          </Fragment>
        )
      })}
      {' · '}updated {usageAge(usage.updated_at)}
    </span>
  )
}
