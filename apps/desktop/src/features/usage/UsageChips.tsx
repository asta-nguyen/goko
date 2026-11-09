import { useEffect, useState } from 'react'
import { cn } from '@/lib/utils'
import { Badge } from '../../components/ui/badge'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '../../components/ui/dropdown-menu'
import { Tooltip, TooltipContent, TooltipTrigger } from '../../components/ui/tooltip'
import { useUi } from '../../stores/ui'
import type { AccountAgent, AgentAccountsView } from '../../services/accounts'
import type { AccountUsage, UsageLimit } from '../../services/usage'
import { AccountUsageSummary } from '../accounts/AccountUsageSummary'
import { useAccounts, useSetActiveAccount } from '../accounts/queries'
import { useUsage } from './queries'
import { resetTime, usageAge, usageToneClass, usageValue, windowLabel } from './tone'

function limitsFor(
  values: Array<UsageLimit | null>,
  provider: 'claude' | 'codex',
): Array<{ label: string; limit: UsageLimit }> {
  return values.flatMap((limit) => (limit ? [{ label: windowLabel(limit.window_minutes, provider), limit }] : []))
}

function AccountMenu({
  agent,
  accounts,
  usageAccounts,
  onPick,
  onManage,
}: {
  agent: AccountAgent
  accounts: AgentAccountsView
  usageAccounts: Record<string, AccountUsage>
  onPick: (id: string) => void
  onManage: () => void
}) {
  return (
    <DropdownMenuContent align="end" className="w-64">
      <DropdownMenuRadioGroup
        value={accounts.active}
        onValueChange={(id) => {
          if (id !== accounts.active) onPick(id)
        }}
      >
        {accounts.items.map((account) => (
          <DropdownMenuRadioItem key={account.id} value={account.id} className="items-start">
            <span className="flex min-w-0 flex-col">
              <span className="truncate">{account.name}</span>
              <AccountUsageSummary agent={agent} usage={usageAccounts[`${agent}:${account.id}`]} />
            </span>
          </DropdownMenuRadioItem>
        ))}
      </DropdownMenuRadioGroup>
      <DropdownMenuSeparator />
      <DropdownMenuItem onSelect={onManage}>Manage accounts…</DropdownMenuItem>
    </DropdownMenuContent>
  )
}

export function UsageChips() {
  const [now, setNow] = useState(() => Date.now())
  const { data } = useUsage()
  const { data: accounts } = useAccounts()
  const setActiveAccount = useSetActiveAccount()
  const openSettings = useUi((state) => state.openSettings)

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
      key: 'claude' as const,
      name: 'Claude',
      updatedAt: data.claude.updated_at,
      limits: limitsFor([data.claude.five_hour, data.claude.seven_day], 'claude'),
    },
    {
      key: 'codex' as const,
      name: 'Codex',
      updatedAt: data.codex.updated_at,
      limits: limitsFor([data.codex.primary, data.codex.secondary], 'codex'),
    },
  ]
    .map((provider) => ({ ...provider, accounts: accounts?.[provider.key] }))
    // With two or more accounts the chip stays even before any usage arrives.
    .filter((provider) => provider.limits.length > 0 || (provider.accounts?.items.length ?? 0) >= 2)

  if (providers.length === 0) return null

  return (
    <div className="flex items-center gap-2">
      {providers.map(({ key, name, updatedAt, limits, accounts: agentAccounts }) => {
        const limitsLabel = limits.map(({ label, limit }) => `${label} ${usageValue(limit, now).text}`).join(' · ')
        const multi = (agentAccounts?.items.length ?? 0) >= 2
        const activeName = multi
          ? agentAccounts?.items.find((item) => item.id === agentAccounts.active)?.name
          : undefined
        const label = [activeName ? `${name} · ${activeName}` : name, limitsLabel].filter(Boolean).join(' ')
        const badge = (
          <Badge
            variant="outline"
            tabIndex={0}
            aria-label={label}
            className={cn('gap-1.5 px-2', multi ? 'cursor-pointer' : 'cursor-help')}
          >
            <span>
              {name}
              {activeName && <span className="text-muted"> · {activeName}</span>}
            </span>
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
        )
        const tip = (
          <TooltipContent side="bottom" className="flex-col items-start gap-0.5">
            {limits.map(({ label: window, limit }) => (
              <div key={window}>{window} resets {resetTime(limit.resets_at)}</div>
            ))}
            <div>Updated {usageAge(updatedAt, now)}</div>
          </TooltipContent>
        )
        if (!multi || agentAccounts === undefined) {
          return (
            <Tooltip key={key}>
              <TooltipTrigger asChild>{badge}</TooltipTrigger>
              {tip}
            </Tooltip>
          )
        }
        return (
          <DropdownMenu key={key}>
            <Tooltip>
              <TooltipTrigger asChild>
                <DropdownMenuTrigger asChild>{badge}</DropdownMenuTrigger>
              </TooltipTrigger>
              {tip}
            </Tooltip>
            <AccountMenu
              agent={key}
              accounts={agentAccounts}
              usageAccounts={data.accounts}
              onPick={(id) => setActiveAccount.mutate({ agent: key, id })}
              onManage={() => openSettings('accounts')}
            />
          </DropdownMenu>
        )
      })}
    </div>
  )
}
