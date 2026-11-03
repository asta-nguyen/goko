import { useQueryClient } from '@tanstack/react-query'
import { AlertCircleIcon } from 'lucide-react'
import { useEffect } from 'react'
import { ThemeToggle } from './components/theme-toggle'
import { Alert, AlertDescription, AlertTitle } from './components/ui/alert'
import { Badge } from './components/ui/badge'
import { TooltipProvider } from './components/ui/tooltip'
import { EditorWorkspace } from './features/editor/EditorWorkspace'
import { ProjectTree } from './features/projects/ProjectTree'
import { useProjectList } from './features/projects/queries'
import { useDaemonStatus } from './features/sessions/queries'
import { TerminalWorkspace } from './features/terminal/TerminalWorkspace'
import { startQuerySync } from './services/sync'
import { checkoutContext, useUi } from './stores/ui'

export default function App() {
  const qc = useQueryClient()
  useEffect(() => startQuerySync(qc), [qc]) // returns its stop function
  const { data: status } = useDaemonStatus()
  const { data: projects } = useProjectList()
  const activeWorkspaceId = useUi((state) => state.activeWorkspaceId)
  const activeView = useUi((state) => state.activeView)
  const workspaces = useUi((state) => state.workspaces)
  const workspaceIds = [...new Set([...Object.keys(workspaces), ...(activeWorkspaceId ? [activeWorkspaceId] : [])])]
  const activeContext = activeWorkspaceId ? checkoutContext(activeWorkspaceId) : null
  const activeProject = activeContext ? (projects?.find((project) => project.id === activeContext.projectId) ?? null) : null
  // Every terminal workspace stays mounted while Editor covers the main area,
  // so tabs, splits, and running sessions survive a view switch untouched.
  const editorMode = activeView === 'editor' && activeContext !== null
  const disconnected = status?.connected === false
  const connection = status ? (disconnected ? 'Disconnected' : 'Connected') : 'Connecting'

  return (
    <TooltipProvider>
      <div className="flex h-screen flex-col bg-bg text-text">
        <header className="flex h-12 shrink-0 items-center gap-3 border-b border-border bg-panel px-4">
          <div className="flex items-center gap-2.5">
            <span className="grid size-7 shrink-0 place-items-center rounded-md bg-accent text-bg" aria-hidden>
              <svg viewBox="112 128 288 256" className="size-4">
                <path
                  d="m142 164 92 92-92 92"
                  fill="none"
                  stroke="currentColor"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  strokeWidth="32"
                />
                <rect x="272" y="332" width="112" height="32" rx="16" fill="currentColor" />
              </svg>
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
            {workspaceIds.map((workspaceId) => (
              <TerminalWorkspace
                key={workspaceId}
                workspaceId={workspaceId}
                active={workspaceId === activeWorkspaceId && !editorMode}
                project={projects?.find((project) => project.id === checkoutContext(workspaceId)?.projectId) ?? null}
                checkoutPath={checkoutContext(workspaceId)?.checkoutPath}
              />
            ))}
            {editorMode && (
              <EditorWorkspace
                project={activeProject}
                projectId={activeContext.projectId}
                checkoutPath={activeContext.checkoutPath}
              />
            )}
            {activeWorkspaceId === null && (
              <div className="flex flex-1 items-center justify-center p-6 text-sm text-muted">
                Select a project to open its terminal workspace.
              </div>
            )}
          </main>
        </div>
      </div>
    </TooltipProvider>
  )
}
