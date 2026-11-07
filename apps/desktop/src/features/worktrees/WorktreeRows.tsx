import { useRef, useState } from 'react'
import { AlertTriangleIcon, ChevronRightIcon, FolderTreeIcon, GitBranchIcon } from 'lucide-react'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { DropdownMenuItem, DropdownMenuSeparator } from '@/components/ui/dropdown-menu'
import { cn } from '@/lib/utils'
import { isBridgeError } from '../../services/errors'
import type { AgentInfo, Project, SessionInfo, WorktreeInfo } from '../../services/types'
import { checkoutForCwd, checkoutKey, useUi } from '../../stores/ui'
import { CustomCommandForm, NewSessionMenu, RowActionsMenu, type SessionTarget } from '../sessions/SessionLaunch'
import { SessionItems } from '../sessions/SessionItems'
import { useRemoveWorktree } from './queries'

const message = (error: unknown) => isBridgeError(error) ? error.message : error instanceof Error ? error.message : String(error)

function Count({ count, label = 'sessions' }: { count: number; label?: string }) {
  return <Badge variant="secondary" aria-label={`${count} ${label}`} className="min-w-6 justify-center px-1.5 tabular-nums">{count}</Badge>
}

const Dot = ({ className, text }: { className: string; text: string }) => (
  <span title={text} className={`size-1.5 shrink-0 rounded-full ${className}`}>
    <span className="sr-only">{text}</span>
  </span>
)

/** Missing checkout -> badge; otherwise a quiet dot for changed / unknown git status. */
export function CheckoutStatus({ checkout }: { checkout: WorktreeInfo }) {
  if (checkout.missing)
    return (
      <Badge variant="warning" className="gap-1">
        <AlertTriangleIcon aria-hidden />
        missing
      </Badge>
    )
  if (checkout.dirty === true)
    return <Dot className="bg-warning" text={`Changed${checkout.changed === null ? '' : ` · ${checkout.changed}`}`} />
  if (checkout.dirty === null) return <Dot className="bg-muted" text="Status unknown" />
  return null
}

/** Row actions appear on hover/focus, or while their menu is open. */
export const REVEAL =
  'opacity-0 transition-opacity group-hover/row:opacity-100 group-focus-within/row:opacity-100 has-[[aria-expanded=true]]:opacity-100'

export function sessionsForCheckout(checkouts: WorktreeInfo[], sessions: SessionInfo[], checkoutPath: string) {
  return sessions.filter((session) => checkoutForCwd(checkouts, session.cwd) === checkoutPath)
}

export function WorktreeRows({
  project,
  worktrees,
  sessions,
  allSessions,
  agents,
  defaultShell,
  selectedId,
  sessionListKnown,
  hasLiveSessions,
  hasUnknownSessionCwd,
  liveCwdSupported,
  disconnected,
  labelOf,
  agentOf,
  onSelect,
  onStart,
  onRemoveFocusFallback,
}: {
  project: Project
  worktrees: WorktreeInfo[]
  sessions: SessionInfo[]
  allSessions: SessionInfo[]
  agents: AgentInfo[]
  defaultShell: string
  selectedId: string | null
  sessionListKnown: boolean
  hasLiveSessions: boolean
  hasUnknownSessionCwd: boolean
  liveCwdSupported: boolean
  disconnected: boolean
  labelOf: (id: string) => string | undefined
  agentOf?: (id: string) => string | undefined
  onSelect: (id: string, workspaceId: string) => void
  onStart: (target: SessionTarget, done?: () => void, checkoutPath?: string) => void
  onRemoveFocusFallback: () => void
}) {
  const remove = useRemoveWorktree()
  const activateWorkspace = useUi((state) => state.activateWorkspace)
  const activeWorkspaceId = useUi((state) => state.activeWorkspaceId)
  const setActiveView = useUi((state) => state.setActiveView)
  const requestEditorNavigation = useUi((state) => state.requestEditorNavigation)
  const [groupExpanded, setGroupExpanded] = useState(true)
  const [expanded, setExpanded] = useState<Record<string, boolean>>({})
  const [confirming, setConfirming] = useState<WorktreeInfo | null>(null)
  const [customPath, setCustomPath] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const removeTrigger = useRef<HTMLButtonElement | null>(null)
  const linked = worktrees.filter((worktree) => !worktree.is_root)

  const confirmRemove = () => {
    if (!confirming) return
    const removing = confirming
    setError(null)
    requestEditorNavigation(async () => {
      try {
        const key = checkoutKey(project.id, removing.path)
        await remove.mutateAsync({ projectId: project.id, path: removing.path })
        if (useUi.getState().activeWorkspaceId === key) activateWorkspace(checkoutKey(project.id, project.root))
        useUi.getState().removeWorkspace(key)
        setConfirming(null)
      } catch (cause) {
        setError(message(cause))
      }
    })
  }

  if (!linked.length) return null

  return (
    <ul className="m-0 ml-4 flex list-none flex-col gap-0.5 border-l border-border p-0 pl-1.5">
      <li>
        <button
          type="button"
          aria-label={`${groupExpanded ? 'Collapse' : 'Expand'} worktrees`}
          aria-expanded={groupExpanded}
          onClick={() => setGroupExpanded((value) => !value)}
          className="flex w-full items-center gap-1 rounded-md px-1 py-0.5 text-left outline-none hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-ring"
        >
          <span className="grid size-7 shrink-0 place-items-center text-muted">
            <ChevronRightIcon className={cn('size-3.5 transition-transform', groupExpanded && 'rotate-90')} aria-hidden />
          </span>
          <span className="flex min-w-0 flex-1 items-center gap-1.5 px-1 py-1.5">
            <FolderTreeIcon className="size-4 shrink-0 text-muted" aria-hidden />
            <span className="min-w-0 flex-1 truncate text-sm font-medium">Worktrees</span>
            <Count count={linked.length} label="worktrees" />
          </span>
        </button>
        {groupExpanded && (
    <ul className="m-0 ml-4 flex list-none flex-col gap-0.5 border-l border-border p-0 pl-1.5">
      {linked.map((worktree) => {
        const checkoutSessions = sessionsForCheckout(worktrees, sessions, worktree.path)
        const attachedSessions = sessionsForCheckout(worktrees, allSessions, worktree.path)
        const isExpanded = expanded[worktree.path] ?? true
        const dirty = worktree.dirty === true
        const unknownStatus = worktree.dirty === null
        const blockedReason = project.missing
          ? 'The project root is missing.'
          : worktree.missing
            ? 'This checkout is missing.'
            : disconnected
              ? 'The daemon is disconnected, so Goko cannot check for attached sessions.'
              : !sessionListKnown
                ? 'Goko has not confirmed the session list yet.'
                : !liveCwdSupported
                  ? 'Update and restart the connected gokod manually before removing worktrees.'
                  : hasLiveSessions
                    ? 'Close all running Goko sessions before removing any worktree.'
                    : hasUnknownSessionCwd
                      ? 'Goko cannot confirm the current directory of every session; close them first.'
                      : unknownStatus
                        ? 'Git could not confirm the checkout status.'
                        : dirty
                          ? 'Commit or discard the checkout changes first.'
                            : attachedSessions.length > 0
                            ? 'Close all sessions in this checkout first.'
                            : null
        const workspaceId = checkoutKey(project.id, worktree.path)
        const branchLabel = worktree.branch ?? `detached ${worktree.head.slice(0, 8)}`
        const custom = customPath === worktree.path
        return (
          <li key={worktree.path}>
            <div className="group/row flex items-center gap-1 rounded-md px-1 py-0.5 hover:bg-bg/60">
              <button
                type="button"
                aria-label={`${isExpanded ? 'Collapse' : 'Expand'} ${worktree.name} sessions`}
                aria-expanded={isExpanded}
                onClick={() => setExpanded((current) => ({ ...current, [worktree.path]: !isExpanded }))}
                className="grid size-7 shrink-0 place-items-center rounded text-muted outline-none hover:bg-bg focus-visible:ring-2 focus-visible:ring-ring"
              >
                <ChevronRightIcon className={cn('size-3.5 transition-transform', isExpanded && 'rotate-90')} aria-hidden />
              </button>
              <button
                type="button"
                aria-label={`${worktree.name} ${worktree.path}`}
                title={worktree.path}
                aria-pressed={activeWorkspaceId === workspaceId}
                onClick={() => activateWorkspace(workspaceId)}
                className={cn('flex min-w-0 flex-1 items-center gap-1.5 rounded px-1 py-1.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring', activeWorkspaceId === workspaceId && 'bg-secondary/70')}
              >
                <GitBranchIcon className="size-4 shrink-0 text-accent" aria-hidden />
                <span className="min-w-0 flex-1 overflow-hidden">
                  <span className="flex min-w-0 items-center gap-1.5 overflow-hidden">
                    <span className="min-w-12 truncate text-sm font-medium">{worktree.name}</span>
                    <CheckoutStatus checkout={worktree} />
                  </span>
                  {branchLabel !== worktree.name && <span dir="ltr" className="block truncate text-[11px] text-muted">{branchLabel}</span>}
                </span>
              </button>
              {/* Shown when collapsed, or when sessions from another project are attached but not listed below. */}
              {attachedSessions.length > 0 && (!isExpanded || attachedSessions.length > checkoutSessions.length) && <Count count={attachedSessions.length} />}
              <span className={cn('flex shrink-0 items-center', REVEAL)}>
                <NewSessionMenu
                  label={`New session in ${project.name} · ${worktree.name}`}
                  agents={agents}
                  disabled={project.missing || worktree.missing || disconnected || remove.isPending}
                  onAgent={(agent) => onStart({ agent }, undefined, worktree.path)}
                  onCustom={() => setCustomPath(worktree.path)}
                />
                <RowActionsMenu ref={removeTrigger} label={`More actions for ${worktree.name}`}>
                  <DropdownMenuItem
                    onSelect={() => {
                      activateWorkspace(workspaceId)
                      setActiveView('editor')
                    }}
                  >
                    Open Editor
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem
                    variant="destructive"
                    textValue={`Remove ${worktree.name}`}
                    disabled={remove.isPending}
                    aria-disabled={Boolean(blockedReason)}
                    className={cn(blockedReason && 'cursor-not-allowed opacity-50')}
                    onSelect={(event) => {
                      if (blockedReason) {
                        event.preventDefault()
                        return
                      }
                      setError(null)
                      setConfirming(worktree)
                    }}
                  >
                    {blockedReason ? `Cannot remove ${worktree.name}: ${blockedReason}` : `Remove ${worktree.name}`}
                  </DropdownMenuItem>
                </RowActionsMenu>
              </span>
            </div>
            {custom && <CustomCommandForm initial={defaultShell} disabled={project.missing || worktree.missing || disconnected || remove.isPending} onStart={(command) => onStart({ command }, () => setCustomPath(null), worktree.path)} onCancel={() => setCustomPath(null)} />}
            {isExpanded && checkoutSessions.length > 0 && (
              <SessionItems sessions={checkoutSessions} labelOf={labelOf} agentOf={agentOf} selectedId={selectedId} workspaceId={workspaceId} onSelect={onSelect} />
            )}
          </li>
        )
      })}
    </ul>
        )}
      </li>

      <Dialog open={confirming !== null} onOpenChange={(open) => { if (!open && !remove.isPending) { setConfirming(null); setError(null) } }}>
        <DialogContent onCloseAutoFocus={(event) => {
          event.preventDefault()
          if (removeTrigger.current?.isConnected && !removeTrigger.current.disabled) removeTrigger.current.focus()
          else onRemoveFocusFallback()
        }}>
          <DialogHeader>
            <DialogTitle>Remove worktree?</DialogTitle>
            <DialogDescription>This deletes the checkout directory and its uncommitted and ignored local files. The branch is preserved.</DialogDescription>
          </DialogHeader>
          {confirming && <p className="m-0 break-all rounded-md bg-bg p-2 font-mono text-xs">{confirming.path}</p>}
          {error && <Alert variant="destructive" role="alert"><AlertDescription>{error}</AlertDescription></Alert>}
          <div className="flex justify-end gap-2">
            <Button variant="ghost" disabled={remove.isPending} onClick={() => setConfirming(null)}>Cancel</Button>
            <Button variant="destructive" disabled={remove.isPending} onClick={() => void confirmRemove()}>{remove.isPending ? 'Removing…' : 'Remove worktree'}</Button>
          </div>
        </DialogContent>
      </Dialog>
    </ul>
  )
}
