import { cn } from '@/lib/utils'
import type { Activity } from '../../services/activity'
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

const ACTIVITY_DOT: Record<Activity, string> = {
  working: 'border-2 border-accent border-t-transparent bg-panel motion-safe:animate-spin',
  waiting: 'bg-danger',
  done: 'bg-success',
}

/** Compact state marker for tree rows; the state text stays available to assistive tech. */
export function StateDot({
  state,
  activity,
  className,
}: {
  state: SessionState
  activity?: Activity | null
  className?: string
}) {
  const shown = state === 'running' ? activity : null
  const label = shown ? `${text(state)} · ${shown}` : text(state)
  return (
    <span
      title={label}
      className={cn(
        'size-2.5 shrink-0 rounded-full',
        shown ? ACTIVITY_DOT[shown] : (DOT[state] ?? 'bg-muted'),
        className,
        // size-3 comes last so tailwind-merge lets it beat the caller's size-2.
        shown === 'working' ? 'size-3' : null,
      )}
    >
      <span className="sr-only">{label}</span>
    </span>
  )
}
