import { useEffect, useState } from 'react'
import { StateBadge } from './StateBadge'
import { useDaemonStatus, useSession, useStopSession } from './queries'

const FORCE_KILL_AFTER_MS = 5000

function format(ms: number) {
  const s = Math.max(0, Math.floor(ms / 1000))
  const mm = String(Math.floor(s / 60) % 60).padStart(2, '0')
  const ss = String(s % 60).padStart(2, '0')
  return s >= 3600 ? `${Math.floor(s / 3600)}:${mm}:${ss}` : `${mm}:${ss}`
}

export function SessionControls({ sessionId }: { sessionId: string }) {
  const { data: session } = useSession(sessionId)
  const { data: status } = useDaemonStatus()
  const stop = useStopSession()

  const state = session?.state
  const live = state === 'created' || state === 'starting' || state === 'running' || state === 'stopping'

  // The tick lives here, so only this component re-renders each second.
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!live) return
    setNow(Date.now())
    const timer = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(timer)
  }, [live, sessionId])

  // ponytail: timed from when this component first sees `stopping`; the daemon sends no stop timestamp.
  const [forceReady, setForceReady] = useState(false)
  useEffect(() => {
    if (state !== 'stopping') return
    const timer = setTimeout(() => setForceReady(true), FORCE_KILL_AFTER_MS)
    return () => {
      clearTimeout(timer)
      setForceReady(false)
    }
  }, [state, sessionId])

  if (!session) return null

  const disabled = !live || status?.connected === false
  const elapsed = (live ? now : (session.ended_at ?? now)) - session.started_at
  const btn = 'rounded border border-border px-2 py-1 text-sm hover:bg-panel disabled:opacity-50'

  return (
    <div className="flex shrink-0 items-center gap-3 border-b border-border bg-panel px-4 py-2">
      <span className="min-w-0 truncate text-sm font-medium">{session.title || session.argv.join(' ')}</span>
      <StateBadge state={session.state} />
      <span className="text-xs tabular-nums text-muted">{format(elapsed)}</span>
      <div className="ml-auto flex items-center gap-2">
        {stop.error && (
          <span role="alert" className="text-xs text-danger">
            {stop.error.message}
          </span>
        )}
        <button type="button" className={btn} disabled={disabled} onClick={() => stop.mutate({ id: sessionId, force: false })}>
          Stop
        </button>
        {forceReady && (
          <button
            type="button"
            className={`${btn} text-danger`}
            disabled={disabled}
            onClick={() => stop.mutate({ id: sessionId, force: true })}
          >
            Force kill
          </button>
        )}
      </div>
    </div>
  )
}
