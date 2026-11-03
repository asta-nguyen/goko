import { useQueryClient } from '@tanstack/react-query'
import { PlusIcon, XIcon } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Separator } from '@/components/ui/separator'
import { cn } from '@/lib/utils'
import { queryKeys } from '../../queryClient'
import { isBridgeError } from '../../services/errors'
import { sessionService } from '../../services/session'
import type { Project, SessionInfo } from '../../services/types'
import { checkoutContext, useUi, UNASSIGNED_WORKSPACE, type PaneLayout, type TerminalPane, type TerminalSplit, type TerminalTab, type TerminalWorkspaceState } from '../../stores/ui'
import { ViewSwitch } from '../editor/ViewSwitch'
import { SessionControls } from '../sessions/SessionControls'
import { useCreateSession, useDaemonStatus, useSessionList } from '../sessions/queries'
import { useWorktreeList } from '../worktrees/queries'
import { TerminalView } from './TerminalView'

const MIN_PANE_SIZE = 180
const SPLITTER_SIZE = 8
type Axis = 'width' | 'height'
const NO_WORKSPACE: TerminalWorkspaceState = {
  tabs: [],
  activeTabId: null,
  defaultPending: false,
  error: null,
}

const errorMessage = (error: unknown) =>
  isBridgeError(error) ? error.message : error instanceof Error ? error.message : String(error)

const finished = (session: SessionInfo) => session.state === 'exited' || session.state === 'failed'
const starting = (session: SessionInfo) => session.state === 'created' || session.state === 'starting'
const collectPanes = (layout: PaneLayout): TerminalPane[] =>
  layout.kind === 'pane' ? [layout] : [...collectPanes(layout.first), ...collectPanes(layout.second)]

const minimumDimension = (layout: PaneLayout, axis: Axis): number => {
  if (layout.kind === 'pane') return MIN_PANE_SIZE
  const first = minimumDimension(layout.first, axis)
  const second = minimumDimension(layout.second, axis)
  const splitAxis: Axis = layout.direction === 'vertical' ? 'width' : 'height'
  return splitAxis === axis ? first + SPLITTER_SIZE + second : Math.max(first, second)
}

function PaneTree({
  layout,
  renderPane,
  onRatio,
}: {
  layout: PaneLayout
  renderPane: (pane: TerminalPane) => React.ReactNode
  onRatio: (split: TerminalSplit, ratio: number) => void
}) {
  if (layout.kind === 'pane') return renderPane(layout)
  return <ResizableSplit layout={layout} onRatio={onRatio} renderPane={renderPane} />
}

function ResizableSplit({
  layout,
  onRatio,
  renderPane,
}: {
  layout: TerminalSplit
  onRatio: (split: TerminalSplit, ratio: number) => void
  renderPane: (pane: TerminalPane) => React.ReactNode
}) {
  const container = useRef<HTMLDivElement>(null)
  const [bounds, setBounds] = useState({ min: 0.05, max: 0.95 })
  const vertical = layout.direction === 'vertical'
  const axis: Axis = vertical ? 'width' : 'height'
  const firstMinimum = minimumDimension(layout.first, axis)
  const secondMinimum = minimumDimension(layout.second, axis)
  const effectiveRatio = Math.max(bounds.min, Math.min(bounds.max, layout.ratio))

  useEffect(() => {
    const update = () => {
      const rect = container.current?.getBoundingClientRect()
      const dimension = vertical ? rect?.width : rect?.height
      if (!dimension) return
      const available = Math.max(1, dimension - SPLITTER_SIZE)
      const min = firstMinimum / available
      const max = 1 - secondMinimum / available
      setBounds(min <= max ? { min, max } : { min: 0.5, max: 0.5 })
    }
    update()
    if (!container.current) return
    const observer = new ResizeObserver(update)
    observer.observe(container.current)
    return () => observer.disconnect()
  }, [firstMinimum, secondMinimum, vertical])

  const ratioAt = (clientX: number, clientY: number) => {
    const rect = container.current?.getBoundingClientRect()
    if (!rect) return layout.ratio
    const dimension = vertical ? rect.width : rect.height
    if (!dimension) return layout.ratio
    const available = Math.max(1, dimension - SPLITTER_SIZE)
    const offset = (vertical ? clientX - rect.left : clientY - rect.top) - SPLITTER_SIZE / 2
    return Math.max(bounds.min, Math.min(bounds.max, offset / available))
  }

  const setFromKeyboard = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const step = event.shiftKey ? 0.1 : 0.025
    let ratio: number | null = null
    if (vertical && event.key === 'ArrowLeft') ratio = effectiveRatio - step
    if (vertical && event.key === 'ArrowRight') ratio = effectiveRatio + step
    if (!vertical && event.key === 'ArrowUp') ratio = effectiveRatio - step
    if (!vertical && event.key === 'ArrowDown') ratio = effectiveRatio + step
    if (event.key === 'Home') ratio = bounds.min
    if (event.key === 'End') ratio = bounds.max
    if (ratio === null) return
    event.preventDefault()
    onRatio(layout, Math.max(bounds.min, Math.min(bounds.max, ratio)))
  }

  return (
    <div
      ref={container}
      className={cn('flex min-h-0 min-w-0 flex-1', vertical ? 'flex-row' : 'flex-col')}
      style={{ minWidth: minimumDimension(layout, 'width'), minHeight: minimumDimension(layout, 'height') }}
    >
      <div
        className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
        style={{
          flex: `${effectiveRatio} 1 0%`,
          minWidth: minimumDimension(layout.first, 'width'),
          minHeight: minimumDimension(layout.first, 'height'),
        }}
      >
        <PaneTree layout={layout.first} renderPane={renderPane} onRatio={onRatio} />
      </div>
      <div
        role="separator"
        aria-label="Resize terminal panes"
        aria-orientation={vertical ? 'vertical' : 'horizontal'}
        aria-valuemin={Math.round(bounds.min * 100)}
        aria-valuemax={Math.round(bounds.max * 100)}
        aria-valuenow={Math.round(effectiveRatio * 100)}
        tabIndex={0}
        onKeyDown={setFromKeyboard}
        onPointerDown={(event) => {
          event.preventDefault()
          event.currentTarget.setPointerCapture?.(event.pointerId)
          onRatio(layout, ratioAt(event.clientX, event.clientY))
        }}
        onPointerMove={(event) => {
          if (event.buttons !== 1) return
          onRatio(layout, ratioAt(event.clientX, event.clientY))
        }}
        className={cn(
          'group relative z-10 shrink-0 touch-none bg-transparent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/50',
          vertical ? 'w-2 cursor-col-resize' : 'h-2 cursor-row-resize',
        )}
      >
        <Separator
          orientation={vertical ? 'vertical' : 'horizontal'}
          className={cn(
            'absolute transition-colors group-hover:bg-accent group-focus-visible:bg-accent',
            vertical ? 'inset-y-0 left-1/2 h-full -translate-x-1/2' : 'inset-x-0 top-1/2 w-full -translate-y-1/2',
          )}
        />
      </div>
      <div
        className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
        style={{
          flex: `${1 - effectiveRatio} 1 0%`,
          minWidth: minimumDimension(layout.second, 'width'),
          minHeight: minimumDimension(layout.second, 'height'),
        }}
      >
        <PaneTree layout={layout.second} renderPane={renderPane} onRatio={onRatio} />
      </div>
    </div>
  )
}

export function TerminalWorkspace({
  workspaceId,
  active,
  project,
  checkoutPath,
}: {
  workspaceId: string
  active: boolean
  project: Project | null
  checkoutPath?: string
}) {
  const queryClient = useQueryClient()
  const sessionsQuery = useSessionList()
  const create = useCreateSession()
  const { data: status } = useDaemonStatus()
  const workspace = useUi((state) => state.workspaces[workspaceId] ?? NO_WORKSPACE)
  const claimDefaultTab = useUi((state) => state.claimDefaultTab)
  const failDefaultTab = useUi((state) => state.failDefaultTab)
  const openSession = useUi((state) => state.openSession)
  const setActiveTab = useUi((state) => state.setActiveTab)
  const focusPane = useUi((state) => state.focusPane)
  const splitPane = useUi((state) => state.splitPane)
  const setSplitRatio = useUi((state) => state.setSplitRatio)
  const beginClosePane = useUi((state) => state.beginClosePane)
  const beginCloseTab = useUi((state) => state.beginCloseTab)
  const markStopRequested = useUi((state) => state.markStopRequested)
  const retryClose = useUi((state) => state.retryClose)
  const setCloseError = useUi((state) => state.setCloseError)
  const removeSession = useUi((state) => state.removeSession)
  const setActiveView = useUi((state) => state.setActiveView)
  const stopped = useRef(new Set<string>())
  const deleting = useRef(new Set<string>())
  const handledActivation = useRef<string | null>(null)
  const [now, setNow] = useState(Date.now())
  const [actionError, setActionError] = useState<string | null>(null)
  const sessionCheckout = checkoutPath ?? project?.root
  const worktreeQuery = useWorktreeList(project?.id ?? '', Boolean(project && checkoutPath))
  const selectedCheckout = worktreeQuery.data?.worktrees.find((item) => item.path === sessionCheckout)
  const checkoutMissing = Boolean(
    checkoutPath && checkoutPath !== project?.root && worktreeQuery.data?.repo && (!selectedCheckout || selectedCheckout.missing),
  )
  const checkoutStatusPending = Boolean(
    checkoutPath && checkoutPath !== project?.root && worktreeQuery.isLoading,
  )

  const ensureDefault = async () => {
    if (workspaceId === UNASSIGNED_WORKSPACE || !project) return
    const current = useUi.getState().workspaces[workspaceId]
    if (current?.tabs.length || current?.defaultPending) return
    if (project.missing) {
      failDefaultTab(workspaceId, 'The project folder is missing. Restore it, then retry.')
      return
    }
    if (checkoutMissing) {
      failDefaultTab(workspaceId, 'This worktree is missing. Restore it or prune the stale Git entry, then refresh the worktree list.')
      return
    }
    if (status?.connected === false) {
      failDefaultTab(workspaceId, 'The daemon is disconnected. Reconnect, then retry.')
      return
    }
    if (!claimDefaultTab(workspaceId)) return
    try {
      const session = await create.mutateAsync({ agent: 'shell', projectId: project.id, checkout: sessionCheckout, cols: 80, rows: 24, title: 'Shell' })
      openSession(workspaceId, session.id, 'Shell', false)
    } catch (error) {
      const projectId = checkoutContext(workspaceId)?.projectId ?? workspaceId
      if (!useUi.getState().removedProjectIds.includes(projectId)) {
        failDefaultTab(workspaceId, errorMessage(error))
      }
    }
  }

  useEffect(() => {
    if (!active) {
      handledActivation.current = null
      return
    }
    if (!project || checkoutStatusPending || handledActivation.current === workspaceId) return
    handledActivation.current = workspaceId
    void ensureDefault()
  }, [active, checkoutMissing, checkoutStatusPending, project?.id, workspaceId])

  const hasClosing = workspace.tabs.some((tab) => collectPanes(tab.layout).some((pane) => pane.closingAt !== null))
  useEffect(() => {
    if (!hasClosing) return
    const timer = setInterval(() => setNow(Date.now()), 500)
    return () => clearInterval(timer)
  }, [hasClosing])

  useEffect(() => {
    const sessions = sessionsQuery.data
    if (!sessions) return
    const byId = new Map(sessions.map((session) => [session.id, session]))
    const pending = workspace.tabs.flatMap((tab) => collectPanes(tab.layout)).filter((pane) => pane.closingAt !== null)

    for (const pane of pending) {
      if (pane.closeError || deleting.current.has(pane.sessionId)) continue
      const session = byId.get(pane.sessionId)
      if (!session || finished(session)) {
        deleting.current.add(pane.sessionId)
        void sessionService.delete(pane.sessionId).then(async () => {
          await queryClient.cancelQueries({ queryKey: queryKeys.sessions })
          queryClient.setQueryData<SessionInfo[]>(queryKeys.sessions, (old = []) => old.filter((item) => item.id !== pane.sessionId))
          await Promise.all([
            queryClient.invalidateQueries({ queryKey: queryKeys.sessions }),
            queryClient.invalidateQueries({ queryKey: queryKeys.sessionMeta }),
          ])
          removeSession(pane.sessionId)
        }).catch((error: unknown) => {
          setCloseError(pane.sessionId, errorMessage(error))
        }).finally(() => deleting.current.delete(pane.sessionId))
        continue
      }
      if (starting(session) || session.state === 'stopping' || pane.stopRequestedAt !== null || stopped.current.has(pane.sessionId)) continue

      stopped.current.add(pane.sessionId)
      markStopRequested(pane.sessionId, Date.now())
      void sessionService.stop(pane.sessionId, false).then(async () => {
        await queryClient.invalidateQueries({ queryKey: queryKeys.sessions })
      }).catch((error: unknown) => {
        setCloseError(pane.sessionId, errorMessage(error))
      }).finally(() => stopped.current.delete(pane.sessionId))
    }
  }, [markStopRequested, queryClient, removeSession, sessionsQuery.data, setCloseError, workspace.tabs])

  const createShell = async (tabTitle: string) => {
    if (!project || project.missing || checkoutMissing || status?.connected === false) return
    setActionError(null)
    try {
      const session = await create.mutateAsync({ agent: 'shell', projectId: project.id, checkout: sessionCheckout, cols: 80, rows: 24, title: tabTitle })
      openSession(workspaceId, session.id, tabTitle)
    } catch (error) {
      setActionError(errorMessage(error))
    }
  }

  const splitActivePane = async (direction: TerminalSplit['direction']) => {
    if (!project || project.missing || checkoutMissing || status?.connected === false || !activeTab || !focusedPane) return
    setActionError(null)
    try {
      const session = await create.mutateAsync({
        agent: 'shell',
        projectId: project.id,
        checkout: sessionCheckout,
        cols: 80,
        rows: 24,
        title: 'Shell',
      })
      if (!splitPane(workspaceId, activeTab.id, focusedPane.id, direction, session.id)) {
        openSession(workspaceId, session.id, 'Shell')
      }
    } catch (error) {
      setActionError(errorMessage(error))
    }
  }

  const closePane = (tab: TerminalTab, pane: TerminalPane) => {
    beginClosePane(workspaceId, tab.id, pane.id, Date.now())
  }

  const retry = (sessionId: string) => {
    stopped.current.delete(sessionId)
    deleting.current.delete(sessionId)
    retryClose(sessionId)
  }

  const activeTab = workspace.tabs.find((tab) => tab.id === workspace.activeTabId) ?? null
  const focusedPane = activeTab ? collectPanes(activeTab.layout).find((pane) => pane.id === activeTab.activePaneId) ?? null : null
  const sessions = sessionsQuery.data ?? []
  const focusedSession = sessions.find((session) => session.id === focusedPane?.sessionId)
  const title = project?.name ?? (workspaceId === UNASSIGNED_WORKSPACE ? 'Unassigned' : 'Project')
  const canCreate = Boolean(project && !project.missing && !checkoutMissing && status?.connected !== false && !workspace.defaultPending)

  const renderPane = (pane: TerminalPane, tab: TerminalTab) => {
    const session = sessions.find((item) => item.id === pane.sessionId)
    const forceReady = pane.stopRequestedAt !== null && session?.state === 'stopping' && now - pane.stopRequestedAt >= 5000
    return (
      <div
        key={pane.id}
        onPointerDown={() => focusPane(workspaceId, tab.id, pane.id)}
        className={cn('relative h-full min-h-0 min-w-0 flex-1', pane.id === tab.activePaneId && 'ring-1 ring-inset ring-accent/40')}
      >
        <TerminalView sessionId={pane.sessionId} active={active && tab.id === workspace.activeTabId} />
        {pane.closingAt !== null && (
          <div className="absolute left-3 top-3 z-20 flex max-w-[min(32rem,90%)] items-center gap-2 rounded-md border border-border bg-panel/95 px-2.5 py-1.5 text-xs shadow-sm">
            <span className="font-medium">Closing</span>
            {pane.closeError && <span role="alert" className="truncate text-danger">{pane.closeError}</span>}
            {pane.closeError && <Button size="xs" variant="outline" onClick={() => retry(pane.sessionId)}>Retry</Button>}
            {forceReady && (
              <Button
                size="xs"
                variant="destructive"
                onClick={() => {
                  retryClose(pane.sessionId)
                  markStopRequested(pane.sessionId, pane.stopRequestedAt ?? Date.now())
                  void sessionService.stop(pane.sessionId, true).then(() => queryClient.invalidateQueries({ queryKey: queryKeys.sessions }))
                    .catch((error: unknown) => setCloseError(pane.sessionId, errorMessage(error)))
                }}
              >
                Force kill
              </Button>
            )}
          </div>
        )}
      </div>
    )
  }

  const renderLayout = (layout: PaneLayout, tab: TerminalTab): React.ReactNode => (
    <PaneTree
      layout={layout}
      renderPane={(pane) => renderPane(pane, tab)}
      onRatio={(split, ratio) => setSplitRatio(workspaceId, tab.id, split.id, ratio)}
    />
  )

  return (
    <section aria-label={`${title} terminal workspace`} className={cn('flex min-h-0 min-w-0 flex-1 flex-col', !active && 'hidden')}>
      <div className="flex min-h-10 shrink-0 items-center gap-2 border-b border-border bg-panel px-2">
        <div role="tablist" aria-label={`${title} terminal tabs`} className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
          {workspace.tabs.map((tab) => (
            <div key={tab.id} className="flex shrink-0 items-center rounded-md bg-bg/50">
              <button
                type="button"
                role="tab"
                aria-selected={tab.id === workspace.activeTabId}
                onClick={() => setActiveTab(workspaceId, tab.id)}
                className={cn(
                  'flex h-8 items-center gap-2 rounded-l-md px-2.5 text-xs outline-none hover:bg-bg focus-visible:ring-2 focus-visible:ring-ring',
                  tab.id === workspace.activeTabId && 'bg-bg font-medium text-text',
                )}
              >
                <span className="max-w-40 truncate">{tab.title}</span>
                {tab.closing && <span className="text-muted">Closing</span>}
              </button>
              <button
                type="button"
                aria-label={`Close ${tab.title} tab`}
                disabled={tab.closing}
                onClick={() => beginCloseTab(workspaceId, tab.id, Date.now())}
                className="grid size-7 place-items-center rounded-r-md text-muted hover:bg-bg hover:text-text focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-40"
              >
                <XIcon className="size-3.5" />
              </button>
            </div>
          ))}
        </div>
        <Button size="sm" variant="ghost" disabled={!canCreate || create.isPending} onClick={() => void createShell(`Shell ${workspace.tabs.length + 1}`)}>
          <PlusIcon data-icon="inline-start" />
          New tab
        </Button>
        <Button size="sm" variant="outline" disabled={!canCreate || !activeTab || !focusedPane || activeTab.closing || create.isPending} onClick={() => void splitActivePane('vertical')}>
          Split left/right
        </Button>
        <Button size="sm" variant="outline" disabled={!canCreate || !activeTab || !focusedPane || activeTab.closing || create.isPending} onClick={() => void splitActivePane('horizontal')}>
          Split top/bottom
        </Button>
        {project && (
          <ViewSwitch view="terminal" editorEnabled={worktreeQuery.data?.repo === true} onChange={setActiveView} />
        )}
      </div>
      {workspace.error && (
        <Alert variant="warning" className="shrink-0 rounded-none border-x-0 border-t-0">
          <AlertDescription className="flex items-center justify-between gap-3">
            <span>{workspace.error}</span>
            <Button size="xs" variant="outline" onClick={() => void ensureDefault()}>Retry</Button>
          </AlertDescription>
        </Alert>
      )}
      {actionError && (
        <Alert variant="destructive" className="shrink-0 rounded-none border-x-0 border-t-0">
          <AlertDescription>{actionError}</AlertDescription>
        </Alert>
      )}
      {focusedPane && focusedSession && activeTab && (
        <SessionControls sessionId={focusedSession.id} onClosePane={() => closePane(activeTab, focusedPane)} />
      )}
      <div className="relative min-h-0 flex-1 overflow-auto bg-bg">
        {workspace.tabs.length === 0 && !workspace.defaultPending ? (
          <div className="flex h-full flex-col items-center justify-center gap-3 p-6 text-center">
            <p className="m-0 text-sm text-muted">{project ? 'No terminal tabs are open in this project.' : 'Open a session from Unassigned.'}</p>
            {project && (
              <Button size="sm" disabled={!canCreate} onClick={() => void createShell('Shell')}>
                <PlusIcon data-icon="inline-start" />
                New terminal
              </Button>
            )}
          </div>
        ) : workspace.defaultPending && workspace.tabs.length === 0 ? (
          <div role="status" className="flex h-full items-center justify-center text-sm text-muted">Starting terminal…</div>
        ) : workspace.tabs.map((tab) => (
          <div
            key={tab.id}
            role="tabpanel"
            aria-label={tab.title}
            hidden={tab.id !== workspace.activeTabId}
            className="absolute inset-0 flex min-h-0 min-w-0"
          >
            {renderLayout(tab.layout, tab)}
          </div>
        ))}
      </div>
    </section>
  )
}
