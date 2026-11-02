import { useQueryClient } from '@tanstack/react-query'
import { AlertCircleIcon, TerminalSquareIcon } from 'lucide-react'
import { useEffect } from 'react'
import { ThemeToggle } from './components/theme-toggle'
import { Alert, AlertDescription, AlertTitle } from './components/ui/alert'
import { Badge } from './components/ui/badge'
import { TooltipProvider } from './components/ui/tooltip'
import { ProjectTree } from './features/projects/ProjectTree'
import { SessionControls } from './features/sessions/SessionControls'
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
  const connection = status ? (disconnected ? 'Disconnected' : 'Connected') : 'Connecting'

  return (
    <TooltipProvider>
      <div className="flex h-screen flex-col bg-bg text-text">
        <header className="flex h-12 shrink-0 items-center gap-3 border-b border-border bg-panel px-4">
          <div className="flex items-center gap-2.5">
            <span className="grid size-7 shrink-0 place-items-center rounded-md bg-accent text-bg" aria-hidden>
              <TerminalSquareIcon className="size-4" />
            </span>
            <span className="text-sm font-semibold tracking-tight">goko</span>
          </div>
          <span
            role="status"
            aria-label={status ? (disconnected ? 'Daemon disconnected' : 'Daemon connected') : 'Daemon status unknown'}
          >
            <Badge variant={status ? (disconnected ? 'destructive' : 'success') : 'outline'} className="gap-1.5">
              <span
                aria-hidden
                className={`size-1.5 rounded-full ${status ? (disconnected ? 'bg-danger' : 'bg-success') : 'bg-muted'}`}
              />
              {connection}
            </Badge>
          </span>
          <div className="ml-auto flex items-center gap-2">
            <ThemeToggle />
          </div>
        </header>
        {disconnected && (
          <Alert variant="warning" className="shrink-0 rounded-none border-x-0 border-t-0">
            <AlertCircleIcon />
            <AlertTitle>Daemon unreachable</AlertTitle>
            <AlertDescription>The daemon is unreachable. Goko is restarting it.</AlertDescription>
          </Alert>
        )}
        <div className="flex min-h-0 flex-1">
          <aside className="flex w-80 shrink-0 flex-col border-r border-border bg-panel">
            <ProjectTree />
          </aside>
          <main className="flex min-w-0 flex-1 flex-col bg-bg">
            {selectedId && <SessionControls sessionId={selectedId} />}
            <TerminalView sessionId={selectedId} />
          </main>
        </div>
      </div>
    </TooltipProvider>
  )
}
