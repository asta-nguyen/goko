import type { SessionState } from '../../services/types'

const TONE: Partial<Record<SessionState, string>> = {
  running: 'text-success',
  created: 'text-warning',
  starting: 'text-warning',
  stopping: 'text-warning',
  failed: 'text-danger',
}

const DOT: Partial<Record<SessionState, string>> = {
  running: 'bg-success',
  created: 'bg-warning',
  starting: 'bg-warning',
  stopping: 'bg-warning',
  failed: 'bg-danger',
}

const text = (state: SessionState) => state.replaceAll('_', ' ')

export function StateBadge({ state }: { state: SessionState }) {
  return (
    <span className={`shrink-0 rounded border border-border px-1.5 py-0.5 text-xs ${TONE[state] ?? 'text-muted'}`}>
      {text(state)}
    </span>
  )
}

/** Compact state marker for tree rows; the state text stays available to assistive tech. */
export function StateDot({ state }: { state: SessionState }) {
  return (
    <span title={text(state)} className={`size-2 shrink-0 rounded-full ${DOT[state] ?? 'bg-muted'}`}>
      <span className="sr-only">{text(state)}</span>
    </span>
  )
}
