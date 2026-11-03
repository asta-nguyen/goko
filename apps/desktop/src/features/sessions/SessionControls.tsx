import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { useProjectList } from '../projects/queries'
import { StateBadge } from './StateBadge'
import { agentLabel, useAgentList, useDaemonStatus, useSession, useSessionMetaList, useStopSession } from './queries'

const FORCE_KILL_AFTER_MS = 5000

function format(ms: number) {
  const s = Math.max(0, Math.floor(ms / 1000))
  const mm = String(Math.floor(s / 60) % 60).padStart(2, '0')
  const ss = String(s % 60).padStart(2, '0')
  return s >= 3600 ? `${Math.floor(s / 3600)}:${mm}:${ss}` : `${mm}:${ss}`
}

export function SessionControls({ sessionId, onClosePane }: { sessionId: string; onClosePane?: () => void }) {
  const { data: session } = useSession(sessionId)
  const { data: status } = useDaemonStatus()
  const stop = useStopSession()
  const { data: metas } = useSessionMetaList()
  const { data: projects } = useProjectList()
  const { data: agents = [] } = useAgentList()

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
  const meta = metas?.find((m) => m.session_id === sessionId)
  const projectName = projects?.find((p) => p.id === meta?.project_id)?.name
  const title = session.title || session.argv.join(' ')
  const agent = meta ? agentLabel(agents, meta.agent) : undefined
  // A Shell session is labelled "Shell" and titled "Shell"; show one, not both.
  const showAgent = agent !== undefined && agent.toLowerCase() !== title.toLowerCase()

  return (
    <div className="flex shrink-0 items-center gap-3 border-b border-border bg-panel px-4 py-2">
      <span className="min-w-0 truncate text-sm">
        {projectName && <span className="text-muted">{projectName} › </span>}
        {showAgent && <span className="text-muted">{agent} · </span>}
        <span className="font-medium">{title}</span>
      </span>
      <StateBadge state={session.state} />
      <span className="text-xs tabular-nums text-muted">{format(elapsed)}</span>
      <div className="ml-auto flex items-center gap-2">
        {stop.error && (
          <span role="alert" className="text-xs text-danger">
            {stop.error.message}
          </span>
        )}
        <Button variant="outline" size="sm" disabled={disabled} onClick={() => stop.mutate({ id: sessionId, force: false })}>
          Stop
        </Button>
        {forceReady && (
          <Button variant="destructive" size="sm" disabled={disabled} onClick={() => stop.mutate({ id: sessionId, force: true })}>
            Force kill
          </Button>
        )}
        {onClosePane && (
          <Button variant="ghost" size="sm" onClick={onClosePane}>
            Close pane
          </Button>
        )}
      </div>
    </div>
  )
}
