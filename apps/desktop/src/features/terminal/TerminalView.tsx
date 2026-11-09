import '@xterm/xterm/css/xterm.css'
import { useQueryClient } from '@tanstack/react-query'
import { openUrl } from '@tauri-apps/plugin-opener'
import { useEffect, useRef } from 'react'
import { queryKeys } from '../../queryClient'
import { gitViewService } from '../../services/git-view'
import { sessionService } from '../../services/session'
import { useUi } from '../../stores/ui'
import { useSession } from '../sessions/queries'
import { TerminalController, type TerminalLinks } from './TerminalController'
import { resolveFileRef } from './terminal-links'

/** Full-height terminal for one session. Output never touches React state; only session metadata does. */
export function TerminalView({
  sessionId,
  active = true,
  workspaceId,
  projectId,
  checkoutPath,
}: {
  sessionId: string | null
  active?: boolean
  workspaceId?: string
  projectId?: string
  checkoutPath?: string
}) {
  const host = useRef<HTMLDivElement>(null)
  const controller = useRef<TerminalController | null>(null)
  const queryClient = useQueryClient()
  const { data: session } = useSession(sessionId)
  // Hover-time reads go through refs so a moved `cwd` or late project context
  // never rebuilds the terminal (a rebuild would drop xterm's UI state).
  const cwdRef = useRef<string | undefined>(undefined)
  useEffect(() => {
    cwdRef.current = session?.cwd
  }, [session?.cwd])
  const contextRef = useRef<{ workspaceId?: string; projectId?: string; checkoutPath?: string }>({})
  useEffect(() => {
    contextRef.current = { workspaceId, projectId, checkoutPath }
  }, [workspaceId, projectId, checkoutPath])

  useEffect(() => {
    if (!sessionId || !host.current) return
    const file: NonNullable<TerminalLinks['file']> = {
      resolve: async (raw) => {
        const { projectId, checkoutPath } = contextRef.current
        if (!projectId || !checkoutPath) return null
        try {
          const files = await queryClient.fetchQuery({
            queryKey: queryKeys.gitFileList(projectId, checkoutPath),
            queryFn: () => gitViewService.fileList(projectId, checkoutPath),
            staleTime: 5000,
          })
          return resolveFileRef({ path: raw }, {
            checkoutPath,
            cwd: cwdRef.current,
            files: new Set(files),
          })
        } catch {
          return null
        }
      },
      open: (path, line) => {
        const { workspaceId } = contextRef.current
        if (workspaceId) useUi.getState().openEditorFile(workspaceId, path, line)
      },
    }
    const links: TerminalLinks = {
      // A rejected open (no browser, denied) must not break the terminal.
      openUrl: (url) => void openUrl(url).catch(() => {}),
      // The provider reads `file` on every hover; the getter keeps it absent
      // until project, checkout, and workspace context have all arrived.
      get file() {
        const { workspaceId, projectId, checkoutPath } = contextRef.current
        return workspaceId && projectId && checkoutPath ? file : undefined
      },
    }
    const instance = new TerminalController(sessionId, sessionService, links)
    controller.current = instance
    void instance.mount(host.current, active)
    return () => {
      instance.dispose()
      if (controller.current === instance) controller.current = null
    }
  }, [sessionId, queryClient])

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
