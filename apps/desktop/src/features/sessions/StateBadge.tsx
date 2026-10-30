import type { SessionState } from '../../services/types'

const TONE: Partial<Record<SessionState, string>> = {
  running: 'text-success',
  created: 'text-warning',
  starting: 'text-warning',
  stopping: 'text-warning',
  failed: 'text-danger',
}

export function StateBadge({ state }: { state: SessionState }) {
  return (
    <span className={`shrink-0 rounded border border-border px-1.5 py-0.5 text-xs ${TONE[state] ?? 'text-muted'}`}>
      {state.replaceAll('_', ' ')}
    </span>
  )
}
