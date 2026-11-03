import { memo } from 'react'
import { cn } from '@/lib/utils'
import type { SessionInfo } from '../../services/types'
import { StateDot } from './StateBadge'

const sessionTitle = (session: SessionInfo) => session.title || session.argv.join(' ')

const SessionRow = memo(function SessionRow({
  session,
  label,
  selected,
  onSelect,
}: {
  session: SessionInfo
  label?: string
  selected: boolean
  onSelect: (id: string) => void
}) {
  return (
    <li>
      <button
        type="button"
        aria-current={selected}
        onClick={() => onSelect(session.id)}
        className={cn(
          'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm outline-none transition-colors hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-ring',
          selected && 'bg-bg shadow-[inset_2px_0_0_var(--accent)]',
        )}
      >
        <StateDot state={session.state} />
        {label && <span className="shrink-0 text-xs font-medium text-muted">{label}</span>}
        <span className="min-w-0 truncate">{sessionTitle(session)}</span>
      </button>
    </li>
  )
})

export function SessionItems({
  sessions,
  labelOf,
  selectedId,
  workspaceId,
  onSelect,
}: {
  sessions: SessionInfo[]
  labelOf: (id: string) => string | undefined
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
          selected={session.id === selectedId}
          onSelect={(id) => onSelect(id, workspaceId)}
        />
      ))}
    </ul>
  )
}
