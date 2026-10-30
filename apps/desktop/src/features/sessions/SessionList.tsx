import { memo } from 'react'
import type { SessionInfo } from '../../services/types'
import { useSelectedId, useUi } from '../../stores/ui'
import { StateBadge } from './StateBadge'
import { useSessionList } from './queries'

// Memoized on session metadata: structural sharing keeps an unchanged session's object identity.
const Row = memo(function Row({
  session,
  selected,
  onSelect,
}: {
  session: SessionInfo
  selected: boolean
  onSelect: (id: string) => void
}) {
  return (
    <li>
      <button
        type="button"
        aria-current={selected}
        onClick={() => onSelect(session.id)}
        className={`flex w-full items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-bg ${
          selected ? 'bg-bg' : ''
        }`}
      >
        <span className="min-w-0 truncate">{session.title || session.argv.join(' ')}</span>
        <StateBadge state={session.state} />
      </button>
    </li>
  )
})

export function SessionList() {
  const { data: sessions = [], error } = useSessionList()
  const selectedId = useSelectedId()
  const select = useUi((s) => s.select)

  if (error) return <p role="alert" className="m-0 text-xs text-danger">{error.message}</p>
  if (sessions.length === 0) return <p className="m-0 text-xs text-muted">No sessions yet.</p>
  return (
    <ul className="m-0 flex min-h-0 list-none flex-col gap-0.5 overflow-y-auto p-0">
      {sessions.map((s) => (
        <Row key={s.id} session={s} selected={s.id === selectedId} onSelect={select} />
      ))}
    </ul>
  )
}
