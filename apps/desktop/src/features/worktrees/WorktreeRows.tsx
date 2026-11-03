import { useRef, useState } from 'react'
import { AlertTriangleIcon, BookOpenIcon, ChevronRightIcon, Trash2Icon } from 'lucide-react'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'
import { isBridgeError } from '../../services/errors'
import type { AgentInfo, Project, SessionInfo, WorktreeInfo } from '../../services/types'
import { checkoutForCwd, checkoutKey, useUi } from '../../stores/ui'
import { CustomCommandForm, NewSessionMenu, type SessionTarget } from '../sessions/SessionLaunch'
import { SessionItems } from '../sessions/SessionItems'
import { useRemoveWorktree } from './queries'

const message = (error: unknown) => isBridgeError(error) ? error.message : error instanceof Error ? error.message : String(error)

function Count({ count }: { count: number }) {
  return <Badge variant="secondary" aria-label={`${count} sessions`} className="min-w-6 justify-center px-1.5 tabular-nums">{count}</Badge>
}

function IconAction({ label, children, ...props }: { label: string; children: React.ReactNode } & React.ComponentProps<typeof Button>) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button variant="ghost" size="icon-sm" aria-label={label} className="text-muted hover:text-text" {...props}>{children}</Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  )
}

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
  onSelect: (id: string, workspaceId: string) => void
  onStart: (target: SessionTarget, done?: () => void, checkoutPath?: string) => void
  onRemoveFocusFallback: () => void
}) {
  const remove = useRemoveWorktree()
  const activateWorkspace = useUi((state) => state.activateWorkspace)
  const activeWorkspaceId = useUi((state) => state.activeWorkspaceId)
  const setActiveView = useUi((state) => state.setActiveView)
  const [expanded, setExpanded] = useState<Record<string, boolean>>({})
  const [confirming, setConfirming] = useState<WorktreeInfo | null>(null)
  const [customPath, setCustomPath] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const removeTrigger = useRef<HTMLButtonElement | null>(null)
  const linked = worktrees.filter((worktree) => !worktree.is_root)

  const confirmRemove = async () => {
    if (!confirming) return
    setError(null)
    try {
      const key = checkoutKey(project.id, confirming.path)
      await remove.mutateAsync({ projectId: project.id, path: confirming.path })
      if (activeWorkspaceId === key) activateWorkspace(checkoutKey(project.id, project.root))
      useUi.getState().removeWorkspace(key)
      setConfirming(null)
    } catch (cause) {
      setError(message(cause))
    }
  }

  if (!linked.length) return null

  return (
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
                aria-pressed={activeWorkspaceId === workspaceId}
                onClick={() => activateWorkspace(workspaceId)}
                className={cn('flex min-w-0 flex-1 items-center gap-1.5 rounded px-1 py-1.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring', activeWorkspaceId === workspaceId && 'bg-secondary/70')}
              >
                <span className="size-2 shrink-0 rounded-full bg-accent" aria-hidden />
                <span className="min-w-0 flex-1">
                  <span className="flex items-center gap-1.5">
                    <span className="truncate text-sm font-medium">{worktree.name}</span>
                    {worktree.missing ? <Badge variant="warning" className="gap-1"><AlertTriangleIcon aria-hidden />missing</Badge> : null}
                    {dirty ? <Badge variant="warning">Changed{worktree.changed === null ? '' : ` · ${worktree.changed}`}</Badge> : null}
                    {!dirty && !unknownStatus && !worktree.missing ? <Badge variant="secondary">Clean</Badge> : null}
                    {unknownStatus && !worktree.missing ? <Badge variant="outline">Status unknown</Badge> : null}
                  </span>
                  <span className="block truncate text-[11px] text-muted" title={worktree.path}>{worktree.branch ?? `detached ${worktree.head.slice(0, 8)}`} · {worktree.path}</span>
                </span>
              </button>
              <Count count={attachedSessions.length} />
              <span className="flex shrink-0 items-center opacity-60 transition-opacity group-hover/row:opacity-100 group-focus-within/row:opacity-100">
                <IconAction
                  label={`Open Editor for ${worktree.name}`}
                  onClick={() => {
                    activateWorkspace(workspaceId)
                    setActiveView('editor')
                  }}
                >
                  <BookOpenIcon />
                </IconAction>
                <NewSessionMenu
                  label={`New session in ${project.name} · ${worktree.name}`}
                  agents={agents}
                  disabled={project.missing || worktree.missing || disconnected || remove.isPending}
                  onAgent={(agent) => onStart({ agent }, undefined, worktree.path)}
                  onCustom={() => setCustomPath(worktree.path)}
                />
                <IconAction
                  label={blockedReason ? `Cannot remove ${worktree.name}: ${blockedReason}` : `Remove ${worktree.name}`}
                  title={blockedReason ?? undefined}
                  disabled={remove.isPending}
                  aria-disabled={Boolean(blockedReason)}
                  className={cn('text-muted hover:text-text', blockedReason && 'cursor-not-allowed opacity-50')}
                  onClick={(event) => {
                    if (blockedReason) return
                    removeTrigger.current = event.currentTarget
                    setError(null)
                    setConfirming(worktree)
                  }}
                >
                  <Trash2Icon />
                </IconAction>
              </span>
            </div>
            {custom && <CustomCommandForm initial={defaultShell} disabled={project.missing || worktree.missing || disconnected || remove.isPending} onStart={(command) => onStart({ command }, () => setCustomPath(null), worktree.path)} onCancel={() => setCustomPath(null)} />}
            {isExpanded && checkoutSessions.length > 0 && (
              <SessionItems sessions={checkoutSessions} labelOf={labelOf} selectedId={selectedId} workspaceId={workspaceId} onSelect={onSelect} />
            )}
          </li>
        )
      })}

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
