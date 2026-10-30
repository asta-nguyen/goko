import { useQueryClient } from '@tanstack/react-query'
import { useEffect } from 'react'
import { NewSessionForm } from './features/sessions/NewSessionForm'
import { SessionControls } from './features/sessions/SessionControls'
import { SessionList } from './features/sessions/SessionList'
import { useDaemonStatus } from './features/sessions/queries'
import { TerminalView } from './features/terminal/TerminalView'
import { startQuerySync } from './services/sync'
import { useSelectedId } from './stores/ui'

export default function App() {
  const qc = useQueryClient()
  useEffect(() => startQuerySync(qc), [qc]) // returns its stop function
  const { data: status } = useDaemonStatus()
  const selectedId = useSelectedId()
  const disconnected = status?.connected === false

  return (
    <div className="flex h-screen flex-col bg-bg text-text">
      <header className="flex h-12 shrink-0 items-center gap-2 border-b border-border bg-panel px-4">
        <span className="text-sm font-semibold tracking-wide">goko</span>
        <span
          role="status"
          aria-label={status ? (disconnected ? 'Daemon disconnected' : 'Daemon connected') : 'Daemon status unknown'}
          className={`size-2 rounded-full ${status ? (disconnected ? 'bg-danger' : 'bg-success') : 'bg-muted'}`}
        />
      </header>
      {disconnected && (
        <div role="alert" className="shrink-0 border-b border-border bg-panel px-4 py-2 text-sm text-warning">
          The daemon is unreachable. Goko is restarting it.
        </div>
      )}
      <div className="flex min-h-0 flex-1">
        <aside className="flex w-72 shrink-0 flex-col gap-4 border-r border-border bg-panel p-3">
          <NewSessionForm />
          <SessionList />
        </aside>
        <main className="flex min-w-0 flex-1 flex-col">
          {selectedId && <SessionControls sessionId={selectedId} />}
          <div className="flex min-h-0 flex-1 flex-col">
            <TerminalView sessionId={selectedId} />
          </div>
        </main>
      </div>
    </div>
  )
}
