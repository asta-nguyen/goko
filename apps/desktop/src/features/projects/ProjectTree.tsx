import { open } from '@tauri-apps/plugin-dialog'
import { useQueryClient } from '@tanstack/react-query'
import { AlertTriangleIcon, BookOpenIcon, ChevronRightIcon, FolderIcon, FolderPlusIcon, PlusIcon, RefreshCwIcon, Trash2Icon } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'
import { queryKeys } from '../../queryClient'
import { isBridgeError } from '../../services/errors'
import type { AgentInfo, Project, SessionInfo, WorktreeInfo } from '../../services/types'
import { checkoutContext, checkoutForCwd, checkoutKey, UNASSIGNED_WORKSPACE, useSelectedId, useUi } from '../../stores/ui'
import { SessionItems } from '../sessions/SessionItems'
import { CustomCommandForm, NewSessionMenu, type SessionTarget } from '../sessions/SessionLaunch'
import {
  agentLabel,
  useAgentList,
  useCreateSession,
  useDaemonStatus,
  useSessionList,
  useSessionMetaList,
} from '../sessions/queries'
import { useAddProject, useProjectList, useRemoveProject } from './queries'
import { useWorktreeList } from '../worktrees/queries'
import { sessionsForCheckout, WorktreeRows } from '../worktrees/WorktreeRows'
import { WorktreeCreateDialog, type WorktreeEnvResult } from '../worktrees/WorktreeCreateDialog'
import { useEnvCopy } from '../worktrees/queries'

type Target = SessionTarget
type Start = (target: Target, done?: () => void, checkoutPath?: string) => void

const message = (e: unknown) => (isBridgeError(e) ? e.message : e instanceof Error ? e.message : String(e))
const sessionTitle = (s: SessionInfo) => s.title || s.argv.join(' ')
const EMPTY_SESSIONS: SessionInfo[] = []

const Count = ({ count }: { count: number }) => (
  <Badge variant="secondary" aria-label={`${count} sessions`} className="min-w-6 justify-center px-1.5 tabular-nums">
    {count}
  </Badge>
)

function IconAction({
  label,
  destructive,
  children,
  ...rest
}: { label: string; destructive?: boolean; children: React.ReactNode } & React.ComponentProps<typeof Button>) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={label}
          className={destructive ? 'text-muted hover:text-danger' : 'text-muted hover:text-text'}
          {...rest}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  )
}

function ProjectNode({
  project,
  sessions,
  allSessions,
  agents,
  defaultShell,
  labelOf,
  selectedId,
  sessionListKnown,
  hasLiveSessions,
  hasUnknownSessionCwd,
  liveCwdSupported,
  active,
  busy,
  disconnected,
  onSelect,
  onActivate,
  onStart,
  onRemove,
}: {
  project: Project
  sessions: SessionInfo[]
  allSessions: SessionInfo[]
  agents: AgentInfo[]
  defaultShell: string
  labelOf: (id: string) => string | undefined
  selectedId: string | null
  sessionListKnown: boolean
  hasLiveSessions: boolean
  hasUnknownSessionCwd: boolean
  liveCwdSupported: boolean
  active: boolean
  busy: boolean
  disconnected: boolean
  onSelect: (id: string, workspaceId: string) => void
  onActivate: () => void
  onStart: Start
  onRemove: () => Promise<void>
}) {
  const [expanded, setExpanded] = useState(true)
  const [confirming, setConfirming] = useState(false)
  const [custom, setCustom] = useState(false)
  const [createWorktreeOpen, setCreateWorktreeOpen] = useState(false)
  const [envResult, setEnvResult] = useState<WorktreeEnvResult | null>(null)
  const row = useRef<HTMLLIElement>(null)
  const wasConfirming = useRef(false)
  const worktreeQuery = useWorktreeList(project.id)
  const retryEnvCopy = useEnvCopy()
  const activeWorkspaceId = useUi((state) => state.activeWorkspaceId)
  const activateWorkspace = useUi((state) => state.activateWorkspace)
  const setActiveView = useUi((state) => state.setActiveView)
  const queryClient = useQueryClient()

  // A selection inside this project (for example a session just created) opens it.
  const hasSelected = sessions.some((s) => s.id === selectedId)
  useEffect(() => {
    if (hasSelected) setExpanded(true)
  }, [hasSelected])

  const worktrees = worktreeQuery.data?.worktrees ?? []
  const root = worktrees.find((item) => item.is_root)
  const isGitProject = worktreeQuery.data?.repo === true
  const rootPath = root?.path ?? project.root
  const rootSessions = isGitProject && root ? sessionsForCheckout(worktrees, sessions, rootPath) : sessions
  const otherSessions = isGitProject
    ? sessions.filter((session) => checkoutForCwd(worktrees, session.cwd) === null)
    : []
  const activeContext = activeWorkspaceId ? checkoutContext(activeWorkspaceId) : null
  const selectedCheckout: WorktreeInfo | undefined = (activeContext?.projectId === project.id
    ? worktrees.find((item) => item.path === activeContext.checkoutPath)
    : undefined) ?? root
  const selectedCheckoutPath = activeContext?.projectId === project.id ? activeContext.checkoutPath : null
  const previous = useRef({ selectedCheckoutPath, sessions })
  useEffect(() => {
    const selectionChanged = previous.current.selectedCheckoutPath !== selectedCheckoutPath
    const sessionsChanged = previous.current.sessions !== sessions
    previous.current = { selectedCheckoutPath, sessions }
    if (selectionChanged || sessionsChanged) {
      void queryClient.invalidateQueries({ queryKey: queryKeys.worktrees(project.id) })
    }
  }, [project.id, queryClient, selectedCheckoutPath, sessions])

  useEffect(() => {
    if (confirming) row.current?.querySelector<HTMLButtonElement>('[data-confirm-cancel]')?.focus()
    else if (wasConfirming.current) row.current?.querySelector<HTMLButtonElement>('[data-row-remove]')?.focus()
    wasConfirming.current = confirming
  }, [confirming])

  if (confirming) {
    return (
      <li ref={row} className="flex flex-col gap-2 rounded-md border border-border bg-bg px-2.5 py-2">
        <span className="text-sm font-medium">Remove {project.name}?</span>
        <span className="text-xs text-muted">Files and running sessions are untouched.</span>
        <span className="flex gap-1.5">
          <Button variant="destructive" size="sm" disabled={busy} onClick={() => void onRemove()}>
            Remove
          </Button>
          <Button data-confirm-cancel variant="ghost" size="sm" disabled={busy} onClick={() => setConfirming(false)}>
            Cancel
          </Button>
        </span>
      </li>
    )
  }

  return (
    <li ref={row}>
      <div className="group/row flex items-center gap-1 rounded-md px-1 py-0.5 transition-colors hover:bg-bg/60">
        <button
          type="button"
          aria-label={`${expanded ? 'Collapse' : 'Expand'} ${project.name} sessions`}
          aria-expanded={expanded}
          onClick={() => setExpanded((v) => !v)}
          className="grid size-7 shrink-0 place-items-center rounded text-muted outline-none hover:bg-bg focus-visible:ring-2 focus-visible:ring-ring"
        >
          <ChevronRightIcon
            className={cn('size-3.5 shrink-0 text-muted transition-transform', expanded && 'rotate-90')}
            aria-hidden
          />
        </button>
        <button
          type="button"
          aria-label={`${project.name} ${project.root}`}
          aria-pressed={active}
          onClick={onActivate}
          className={cn(
            'flex min-w-0 flex-1 items-center gap-1.5 rounded px-1 py-1.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring',
            active && 'bg-secondary/70',
          )}
        >
          <FolderIcon className="size-4 shrink-0 text-accent" aria-hidden />
          <span className="min-w-0 flex-1">
            <span className="flex items-center gap-1.5">
              <span className="truncate text-sm font-medium">{project.name}</span>
              {isGitProject && root && <Badge variant="outline">root</Badge>}
              {root?.branch ? <Badge variant="secondary">{root.branch}</Badge> : null}
              {isGitProject && root?.branch === null && root && <Badge variant="secondary">detached {root.head.slice(0, 8)}</Badge>}
              {root?.dirty === true && <Badge variant="warning">Changed{root.changed === null ? '' : ` · ${root.changed}`}</Badge>}
              {root?.dirty === false && !root.missing && <Badge variant="secondary">Clean</Badge>}
              {root?.dirty === null && !root.missing && <Badge variant="outline">Status unavailable</Badge>}
              {project.missing && (
                <Badge variant="warning" className="gap-1">
                  <AlertTriangleIcon aria-hidden />
                  missing
                </Badge>
              )}
            </span>
            <span
              className="block truncate text-[11px] text-muted"
              title={project.root}
              style={{ direction: 'rtl', textAlign: 'left' }}
            >
              {project.root}
            </span>
          </span>
        </button>
        <Count count={isGitProject ? rootSessions.length : sessions.length} />
        <span className="flex shrink-0 items-center opacity-60 transition-opacity group-hover/row:opacity-100 group-focus-within/row:opacity-100">
          {isGitProject && root && (
            <IconAction
              label={`Open Editor for ${project.name}`}
              onClick={() => {
                activateWorkspace(checkoutKey(project.id, rootPath))
                setActiveView('editor')
              }}
            >
              <BookOpenIcon />
            </IconAction>
          )}
          <NewSessionMenu
            label={`New session in ${project.name}`}
            agents={agents}
            disabled={project.missing || busy || disconnected}
            onAgent={(agent) => onStart({ agent })}
            onCustom={() => setCustom(true)}
          />
          {isGitProject && (
            <IconAction
              label={`Create worktree in ${project.name}`}
              disabled={project.missing || !selectedCheckout}
              onClick={() => setCreateWorktreeOpen(true)}
            >
              <FolderPlusIcon />
            </IconAction>
          )}
          {isGitProject && (
            <IconAction label={`Refresh worktrees for ${project.name}`} onClick={() => void worktreeQuery.refetch()}>
              <RefreshCwIcon className={worktreeQuery.isFetching ? 'animate-spin' : ''} />
            </IconAction>
          )}
          <IconAction label={`Remove ${project.name}`} data-row-remove destructive disabled={busy} onClick={() => setConfirming(true)}>
            <Trash2Icon />
          </IconAction>
        </span>
      </div>
      {custom && (
        <CustomCommandForm
          initial={defaultShell}
          disabled={project.missing || disconnected}
          onStart={(command) => onStart({ command }, () => setCustom(false))}
          onCancel={() => setCustom(false)}
        />
      )}
      {worktreeQuery.error && <p role="alert" className="ml-10 m-0 text-xs text-danger">{worktreeQuery.error.message}</p>}
      {expanded && isGitProject && root && (
        <WorktreeRows
          project={project}
          worktrees={worktrees}
          sessions={sessions}
          allSessions={allSessions}
          agents={agents}
          defaultShell={defaultShell}
          selectedId={selectedId}
          disconnected={disconnected}
          labelOf={labelOf}
          onSelect={onSelect}
          onStart={onStart}
          onRemoveFocusFallback={() => row.current?.querySelector<HTMLButtonElement>('button[aria-pressed]')?.focus()}
          sessionListKnown={sessionListKnown}
          hasLiveSessions={hasLiveSessions}
          hasUnknownSessionCwd={hasUnknownSessionCwd}
          liveCwdSupported={liveCwdSupported}
        />
      )}
      {expanded && rootSessions.length > 0 && (
        <SessionItems
          sessions={rootSessions}
          labelOf={labelOf}
          selectedId={selectedId}
          workspaceId={checkoutKey(project.id, project.root)}
          onSelect={onSelect}
        />
      )}
      {expanded && otherSessions.length > 0 && (
        <div className="ml-4 border-l border-border pl-1.5">
          <div className="flex items-center gap-1.5 px-2 pb-1 pt-1 text-[11px] font-medium text-muted">
            Other sessions <Count count={otherSessions.length} />
          </div>
          <SessionItems
            sessions={otherSessions}
            labelOf={labelOf}
            selectedId={selectedId}
            workspaceId={checkoutKey(project.id, project.root)}
            onSelect={onSelect}
          />
        </div>
      )}
      {isGitProject && selectedCheckout && (
        <WorktreeCreateDialog
          project={project}
          worktrees={worktrees}
          selectedCheckout={selectedCheckout}
          open={createWorktreeOpen}
          onOpenChange={setCreateWorktreeOpen}
          onFocusReturn={() => row.current?.querySelector<HTMLButtonElement>('[aria-label^="Create worktree in"]')?.focus()}
          onCreated={(worktree, result) => {
            setEnvResult(result)
            activateWorkspace(checkoutKey(project.id, worktree.path))
            setActiveView('terminal')
          }}
        />
      )}
      {envResult && (
        <Alert className="ml-10 mt-2 py-2">
          <AlertDescription>
            {envResult.error
              ? `Worktree created, but environment files could not be copied from ${envResult.sourcePath}: ${envResult.error}`
              : envResult.report?.failed.length
                ? `Worktree created; some environment files could not be copied from ${envResult.sourcePath}.`
                : `Environment copy from ${envResult.sourcePath} completed.`}
            {envResult.report && (
              <ul className="m-0 mt-1 list-disc pl-5 text-xs">
                {envResult.report.copied.map((name) => <li key={`copied:${name}`}>{name}: copied</li>)}
                {envResult.report.skipped.map((name) => <li key={`skipped:${name}`}>{name}: skipped (already exists or excluded)</li>)}
                {envResult.report.failed.map((item) => <li key={`failed:${item.name}`}>{item.name}: {item.error}</li>)}
                {!envResult.report.copied.length && !envResult.report.skipped.length && !envResult.report.failed.length && <li>No environment files found.</li>}
              </ul>
            )}
            <span className="ml-2 inline-flex gap-1">
              {Boolean(envResult.error || envResult.report?.failed.length) && (
                <Button
                  variant="outline"
                  size="sm"
                  className="h-6 px-1.5"
                  disabled={retryEnvCopy.isPending}
                  onClick={() => {
                    void retryEnvCopy.mutateAsync({ projectId: project.id, sourcePath: envResult.sourcePath, targetPath: envResult.targetPath })
                      .then((report) => setEnvResult({ ...envResult, report, error: null }))
                      .catch((error: unknown) => setEnvResult({ ...envResult, report: null, error: message(error) }))
                  }}
                >
                  {retryEnvCopy.isPending ? 'Retrying…' : 'Retry copy'}
                </Button>
              )}
              <Button variant="ghost" size="sm" className="h-6 px-1" onClick={() => setEnvResult(null)}>Dismiss</Button>
            </span>
          </AlertDescription>
        </Alert>
      )}
    </li>
  )
}

/** The sidebar: every project as a parent node with its agent sessions nested beneath. */
export function ProjectTree() {
  const { data: projects, error: projectsError } = useProjectList()
  const sessionsQuery = useSessionList()
  const { data: sessions = [], error: sessionsError } = sessionsQuery
  const sessionMetaQuery = useSessionMetaList()
  const { data: metas = [] } = sessionMetaQuery
  const { data: agents = [] } = useAgentList()
  const { data: status } = useDaemonStatus()
  const selectedId = useSelectedId()
  const activeWorkspaceId = useUi((s) => s.activeWorkspaceId)
  const openSession = useUi((s) => s.openSession)
  const activateWorkspace = useUi((s) => s.activateWorkspace)
  const add = useAddProject()
  const remove = useRemoveProject()
  const create = useCreateSession()
  const [actionError, setActionError] = useState<string | null>(null)
  const addButton = useRef<HTMLButtonElement>(null)
  const inFlight = useRef(false) // `isPending` lags a render, so a fast double-click needs this

  const metaById = useMemo(() => new Map(metas.map((m) => [m.session_id, m])), [metas])
  const hasLiveSessions = sessions.some((session) => session.state !== 'exited' && session.state !== 'failed')
  const hasUnknownSessionCwd = sessions.some((session) => !session.cwd)
  const liveCwdSupported = status?.live_cwd_supported !== false
  const { byProject, unassigned } = useMemo(() => {
    const known = new Set(projects?.map((p) => p.id))
    const byProject: Record<string, SessionInfo[]> = {}
    const unassigned: SessionInfo[] = []
    for (const s of sessions) {
      const id = metaById.get(s.id)?.project_id
      if (id != null && known.has(id)) (byProject[id] ??= []).push(s)
      else unassigned.push(s)
    }
    return { byProject, unassigned }
  }, [metaById, projects, sessions])

  const labelOf = (sessionId: string) => {
    const agent = metaById.get(sessionId)?.agent
    return agent === undefined ? undefined : agentLabel(agents, agent)
  }

  function start(project: Project, target: Target, done?: () => void, checkoutPath = project.root) {
    if (inFlight.current || disconnected) return
    inFlight.current = true
    const workspaceId = checkoutKey(project.id, checkoutPath)
    create.mutate(
      { ...target, projectId: project.id, checkout: checkoutPath, cols: 80, rows: 24 },
      {
        onSuccess: (session) => {
          openSession(workspaceId, session.id, sessionTitle(session))
          done?.()
        },
        onSettled: () => (inFlight.current = false),
      },
    )
  }

  async function addProject() {
    setActionError(null)
    try {
      const dir = await open({ directory: true })
      if (typeof dir !== 'string') return // cancelled: nothing changes
      await add.mutateAsync(dir)
    } catch (e) {
      setActionError(message(e))
    }
  }

  async function removeProject(project: Project) {
    setActionError(null)
    try {
      await remove.mutateAsync(project.id)
      useUi.getState().removeProject(project.id)
      addButton.current?.focus() // the removed row had focus
    } catch (e) {
      setActionError(message(e))
    }
  }

  const busy = add.isPending || remove.isPending || create.isPending
  const disconnected = status?.connected === false
  const defaultShell = status?.default_shell ?? ''
  const selectSession = (id: string, workspaceId: string) => {
    const session = sessions.find((item) => item.id === id)
    if (session) openSession(workspaceId, id, sessionTitle(session))
  }

  return (
    <TooltipProvider>
      <section aria-label="Projects" className="flex min-h-0 flex-1 flex-col gap-2 p-3">
        <header className="flex items-center justify-between px-1">
          <h2 className="m-0 text-[11px] font-semibold uppercase tracking-wider text-muted">Projects</h2>
          <Button ref={addButton} variant="ghost" size="sm" onClick={() => void addProject()} disabled={add.isPending}>
            <PlusIcon data-icon="inline-start" />
            Add Project
          </Button>
        </header>
        {projectsError && (
          <Alert variant="destructive" className="px-2.5 py-2">
            <AlertDescription>{projectsError.message}</AlertDescription>
          </Alert>
        )}
        {projects !== undefined && projects.length === 0 && sessions.length === 0 ? (
          <Empty className="p-4">
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <FolderPlusIcon />
              </EmptyMedia>
              <EmptyTitle>No projects yet</EmptyTitle>
              <EmptyDescription>Add a project folder, then start an agent inside it.</EmptyDescription>
            </EmptyHeader>
          </Empty>
        ) : (
          <ScrollArea className="min-h-0 flex-1">
            <ul className="m-0 flex list-none flex-col gap-1 p-0 pr-2">
              {projects?.map((project) => (
                <ProjectNode
                  key={project.id}
                  project={project}
                  sessions={byProject[project.id] ?? EMPTY_SESSIONS}
                  allSessions={sessions}
                  agents={agents}
                  defaultShell={defaultShell}
                  active={activeWorkspaceId === checkoutKey(project.id, project.root)}
                  busy={busy}
                  disconnected={disconnected}
                  onActivate={() => activateWorkspace(checkoutKey(project.id, project.root))}
                  onStart={(target, done, checkoutPath) => start(project, target, done, checkoutPath)}
                  onRemove={() => removeProject(project)}
                  labelOf={labelOf}
                  selectedId={selectedId}
                  sessionListKnown={sessionsQuery.data !== undefined && !sessionsError && sessionMetaQuery.data !== undefined && !sessionMetaQuery.error}
                  hasLiveSessions={hasLiveSessions}
                  hasUnknownSessionCwd={hasUnknownSessionCwd}
                  liveCwdSupported={liveCwdSupported}
                  onSelect={selectSession}
                />
              ))}
              {unassigned.length > 0 && (
                <li>
                  <div className="flex items-center gap-1.5 px-2 pb-1 pt-2 text-[11px] font-semibold uppercase tracking-wider text-muted">
                    <button
                      type="button"
                      aria-pressed={activeWorkspaceId === UNASSIGNED_WORKSPACE}
                      onClick={() => activateWorkspace(UNASSIGNED_WORKSPACE)}
                      className="flex-1 rounded text-left outline-none hover:text-text focus-visible:ring-2 focus-visible:ring-ring"
                    >
                      Unassigned
                    </button>
                    <Count count={unassigned.length} />
                  </div>
                  <SessionItems
                    sessions={unassigned}
                    labelOf={labelOf}
                    selectedId={selectedId}
                    workspaceId={UNASSIGNED_WORKSPACE}
                    onSelect={selectSession}
                  />
                </li>
              )}
            </ul>
          </ScrollArea>
        )}
        {sessionsError && (
          <Alert variant="destructive" className="px-2.5 py-2">
            <AlertDescription>{sessionsError.message}</AlertDescription>
          </Alert>
        )}
        {actionError && (
          <Alert variant="destructive" className="px-2.5 py-2">
            <AlertDescription>{actionError}</AlertDescription>
          </Alert>
        )}
        {create.error && (
          <p role="alert" className="m-0 text-xs text-danger">
            {create.error.message}
            {create.error.hint && <span className="block text-muted">{create.error.hint}</span>}
          </p>
        )}
      </section>
    </TooltipProvider>
  )
}
