import { memo } from 'react'
import { cn } from '@/lib/utils'
import { accountNameOf } from '../../services/accounts'
import type { Subagent } from '../../services/subagents'
import type { SessionInfo } from '../../services/types'
import { useAccounts } from '../accounts/queries'
import { useSessionActivity } from '../activity/queries'
import { useSessionSubagents } from '../subagents/queries'
import { useSessionContext } from '../usage/queries'
import { usageTone, usageToneClass } from '../usage/tone'
import { AgentIcon } from './AgentIcon'
import { StateDot } from './StateBadge'

const sessionTitle = (session: SessionInfo) => session.title || session.argv.join(' ')

/** `↳ devin`, `↳ devin ×2`, `↳ codex, devin ×2`: agent names alphabetical, counts after the name. */
function SubagentIndicator({ subagents }: { subagents: Subagent[] }) {
  const byAgent = [...subagents].sort((a, b) => a.agent.localeCompare(b.agent))
  const counts = new Map<string, number>()
  for (const { agent } of byAgent) counts.set(agent, (counts.get(agent) ?? 0) + 1)
  const names = [...counts.entries()].map(([agent, n]) => (n > 1 ? `${agent} ×${n}` : agent)).join(', ')
  const entries = byAgent.map(({ agent, pid, command }) => `${agent} (pid ${pid}): ${command}`)
  return (
    <span title={entries.join('\n')} className="min-w-0 truncate text-xs text-muted">
      <span aria-hidden="true">{`↳ ${names}`}</span>
      <span className="sr-only">{`Running sub-agents: ${entries.join('; ')}`}</span>
    </span>
  )
}

const SessionRow = memo(function SessionRow({
  session,
  label,
  agent,
  selected,
  indent,
  onSelect,
}: {
  session: SessionInfo
  label?: string
  agent?: string
  selected: boolean
  indent: boolean
  onSelect: (id: string) => void
}) {
  const title = sessionTitle(session)
  // A Shell session is labelled "Shell" and titled "Shell"; show one, not both.
  const showLabel = label !== undefined && label.toLowerCase() !== title.toLowerCase()
  const ended = session.state === 'exited' || session.state === 'failed'
  const contextPct = useSessionContext(session)
  const context = contextPct === null ? null : Math.round(contextPct)
  const activity = useSessionActivity(session)
  const subagents = useSessionSubagents(session)
  const { data: accounts } = useAccounts()
  // The account comes from the session's own argv, so it survives active-account switches.
  const agentKey = agent === 'claude' || agent === 'codex' ? agent : null
  const agentAccounts = agentKey === null ? undefined : accounts?.[agentKey]
  const accountName =
    agentKey !== null && agentAccounts !== undefined && agentAccounts.items.length >= 2
      ? accountNameOf(agentKey, session.argv, accounts)
      : null
  return (
    <li>
      <button
        type="button"
        aria-current={selected}
        onClick={() => onSelect(session.id)}
        className={cn(
          'flex w-full items-center gap-2 rounded-md py-1.5 text-left text-sm outline-none transition-colors hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-ring',
          // pl-10 puts the icon under a folder row's icon (chevron cell + gap), so sessions read as its children.
          indent ? 'pl-10 pr-2' : 'px-2',
          ended && 'text-muted',
          selected && 'bg-accent/10 text-accent',
        )}
      >
        <span className="relative grid size-4 shrink-0 place-items-center">
          <AgentIcon agent={agent} />
          <StateDot
            state={session.state}
            activity={activity}
            className="absolute -bottom-0.5 -right-0.5 size-2 ring-2 ring-panel"
          />
        </span>
        {showLabel ? (
          <>
            <span className="shrink-0 font-medium">{label}</span>
            <span className="min-w-0 truncate text-xs text-muted">{title}</span>
          </>
        ) : (
          <span className="min-w-0 truncate font-medium">{title}</span>
        )}
        {accountName !== null && (
          <>
            {' '}
            <span className="min-w-0 truncate text-xs text-muted">· {accountName}</span>
          </>
        )}
        {subagents.length > 0 && <SubagentIndicator subagents={subagents} />}
        {contextPct !== null && (
          <span
            aria-label={`Claude context ${context}%`}
            className={cn('ml-auto shrink-0 text-xs tabular-nums', usageToneClass[usageTone(contextPct)])}
          >
            {context}%
          </span>
        )}
      </button>
    </li>
  )
})

export function SessionItems({
  sessions,
  labelOf,
  agentOf,
  selectedId,
  workspaceId,
  indent = true,
  onSelect,
}: {
  sessions: SessionInfo[]
  labelOf: (id: string) => string | undefined
  agentOf?: (id: string) => string | undefined
  selectedId: string | null
  workspaceId: string
  /** Indent rows under a folder row; off for a list with no folder parent. */
  indent?: boolean
  onSelect: (id: string, workspaceId: string) => void
}) {
  return (
    <ul className="m-0 ml-4 flex list-none flex-col gap-0.5 border-l border-border p-0 pl-1.5">
      {sessions.map((session) => (
        <SessionRow
          key={session.id}
          session={session}
          label={labelOf(session.id)}
          agent={agentOf?.(session.id)}
          selected={session.id === selectedId}
          indent={indent}
          onSelect={(id) => onSelect(id, workspaceId)}
        />
      ))}
    </ul>
  )
}
