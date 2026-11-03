import '@xterm/xterm/css/xterm.css'
import { useEffect, useRef } from 'react'
import { sessionService } from '../../services/session'
import { useSession } from '../sessions/queries'
import { TerminalController } from './TerminalController'

/** Full-height terminal for one session. Output never touches React state; only session metadata does. */
export function TerminalView({ sessionId, active = true }: { sessionId: string | null; active?: boolean }) {
  const host = useRef<HTMLDivElement>(null)
  const controller = useRef<TerminalController | null>(null)
  const { data: session } = useSession(sessionId)

  useEffect(() => {
    if (!sessionId || !host.current) return
    const instance = new TerminalController(sessionId, sessionService)
    controller.current = instance
    void instance.mount(host.current, active)
    return () => {
      instance.dispose()
      if (controller.current === instance) controller.current = null
    }
  }, [sessionId])

  useEffect(() => controller.current?.setVisible(active), [active])

  if (!sessionId) {
    return (
      <div className="flex h-full items-center justify-center bg-bg p-6 text-sm text-muted">
        Select or create a session to see its terminal.
      </div>
    )
  }

  return (
    <div className="flex h-full min-h-0 flex-col bg-bg">
      {session?.state === 'exited' && (
        <div role="status" className="shrink-0 border-b border-border bg-panel px-4 py-2 text-sm text-muted">
          {session.exit_code === null ? 'Process exited' : `Process exited with code ${session.exit_code}`}
        </div>
      )}
      {session?.state === 'failed' && (
        <div role="alert" className="shrink-0 border-b border-border bg-panel px-4 py-2 text-sm text-danger">
          Failed to start{session.error ? `: ${session.error}` : ''}
        </div>
      )}
      <div className="relative min-h-0 flex-1">
        <div ref={host} className="absolute inset-2" />
      </div>
    </div>
  )
}
