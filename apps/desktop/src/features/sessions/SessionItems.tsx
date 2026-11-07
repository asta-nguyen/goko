import { memo } from 'react'
import { cn } from '@/lib/utils'
import type { SessionInfo } from '../../services/types'
import { useSessionActivity } from '../activity/queries'
import { useSessionContext } from '../usage/queries'
import { usageTone, usageToneClass } from '../usage/tone'
import { AgentIcon } from './AgentIcon'
import { StateDot } from './StateBadge'

const sessionTitle = (session: SessionInfo) => session.title || session.argv.join(' ')

const SessionRow = memo(function SessionRow({
  session,
  label,
  agent,
  selected,
  onSelect,
}: {
  session: SessionInfo
  label?: string
  agent?: string
  selected: boolean
  onSelect: (id: string) => void
}) {
  const title = sessionTitle(session)
  // A Shell session is labelled "Shell" and titled "Shell"; show one, not both.
  const showLabel = label !== undefined && label.toLowerCase() !== title.toLowerCase()
  const ended = session.state === 'exited' || session.state === 'failed'
  const contextPct = useSessionContext(session)
  const context = contextPct === null ? null : Math.round(contextPct)
  const activity = useSessionActivity(session)
  return (
    <li>
      <button
        type="button"
        aria-current={selected}
        onClick={() => onSelect(session.id)}
        className={cn(
          'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm outline-none transition-colors hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-ring',
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
  onSelect,
}: {
  sessions: SessionInfo[]
  labelOf: (id: string) => string | undefined
  agentOf?: (id: string) => string | undefined
  selectedId: string | null
  workspaceId: string
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
          onSelect={(id) => onSelect(id, workspaceId)}
        />
      ))}
    </ul>
  )
}
