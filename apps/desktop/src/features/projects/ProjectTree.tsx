import { open } from '@tauri-apps/plugin-dialog'
import { AlertTriangleIcon, ChevronRightIcon, FolderIcon, FolderPlusIcon, PlusIcon, Trash2Icon } from 'lucide-react'
import { type FormEvent, memo, useEffect, useMemo, useRef, useState } from 'react'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'
import { isBridgeError } from '../../services/errors'
import type { AgentInfo, Project, SessionInfo } from '../../services/types'
import { useSelectedId, useUi } from '../../stores/ui'
import {
  agentLabel,
  useAgentList,
  useCreateSession,
  useDaemonStatus,
  useSessionList,
  useSessionMetaList,
} from '../sessions/queries'
import { StateDot } from '../sessions/StateBadge'
import { useAddProject, useProjectList, useRemoveProject } from './queries'

type Target = { agent: string } | { command: string }
type Start = (target: Target, done?: () => void) => void

const message = (e: unknown) => (isBridgeError(e) ? e.message : e instanceof Error ? e.message : String(e))
const sessionTitle = (s: SessionInfo) => s.title || s.argv.join(' ')

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

// Memoized on session identity: structural sharing keeps an unchanged session's object.
const SessionRow = memo(function SessionRow({
  session,
  label,
  selected,
  onSelect,
}: {
  session: SessionInfo
  label?: string
  selected: boolean
  onSelect: (id: string) => void
}) {
  return (
    <li>
      <button
        type="button"
        aria-current={selected}
        onClick={() => onSelect(session.id)}
        className={cn(
          'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm outline-none transition-colors hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-ring',
          selected && 'bg-bg shadow-[inset_2px_0_0_var(--accent)]',
        )}
      >
        <StateDot state={session.state} />
        {label && <span className="shrink-0 text-xs font-medium text-muted">{label}</span>}
        <span className="min-w-0 truncate">{sessionTitle(session)}</span>
      </button>
    </li>
  )
})

function SessionItems({
  sessions,
  labelOf,
  selectedId,
  onSelect,
}: {
  sessions: SessionInfo[]
  labelOf: (id: string) => string | undefined
  selectedId: string | null
  onSelect: (id: string) => void
}) {
  return (
    <ul className="m-0 ml-4 flex list-none flex-col gap-0.5 border-l border-border p-0 pl-1.5">
      {sessions.map((s) => (
        <SessionRow key={s.id} session={s} label={labelOf(s.id)} selected={s.id === selectedId} onSelect={onSelect} />
      ))}
    </ul>
  )
}

function NewSessionMenu({
  project,
  agents,
  disabled,
  onAgent,
  onCustom,
}: {
  project: Project
  agents: AgentInfo[]
  disabled: boolean
  onAgent: (id: string) => void
  onCustom: () => void
}) {
  const label = `New session in ${project.name}`
  return (
    <DropdownMenu>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={label}
              disabled={disabled}
              className="text-muted hover:text-text"
            >
              <PlusIcon />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent>{label}</TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end" className="w-56">
        {agents.map((a) => (
          <DropdownMenuItem key={a.id} disabled={!a.available} onSelect={() => onAgent(a.id)}>
            {a.label}
            {!a.available && ' — not found on PATH'}
          </DropdownMenuItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={onCustom}>Custom command…</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

function CustomCommandForm({
  initial,
  onStart,
  onCancel,
}: {
  initial: string
  onStart: (command: string) => void
  onCancel: () => void
}) {
  const [typed, setTyped] = useState<string | null>(null) // null = untouched, show the default shell
  const command = typed ?? initial
  const submit = (e: FormEvent) => {
    e.preventDefault()
    if (command.trim() !== '') onStart(command)
  }
  return (
    <form onSubmit={submit} className="ml-4 flex flex-col gap-1.5 border-l border-border py-1 pl-2">
      <Input
        aria-label="Command"
        autoFocus
        value={command}
        onChange={(e) => setTyped(e.target.value)}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        className="font-mono"
      />
      <span className="flex gap-1.5">
        <Button type="submit" size="sm" disabled={command.trim() === ''}>
          Start
        </Button>
        <Button type="button" variant="ghost" size="sm" onClick={onCancel}>
          Cancel
        </Button>
      </span>
    </form>
  )
}

function ProjectNode({
  project,
  sessions,
  agents,
  defaultShell,
  labelOf,
  selectedId,
  busy,
  onSelect,
  onStart,
  onRemove,
}: {
  project: Project
  sessions: SessionInfo[]
  agents: AgentInfo[]
  defaultShell: string
  labelOf: (id: string) => string | undefined
  selectedId: string | null
  busy: boolean
  onSelect: (id: string) => void
  onStart: Start
  onRemove: () => Promise<void>
}) {
  const [expanded, setExpanded] = useState(true)
  const [confirming, setConfirming] = useState(false)
  const [custom, setCustom] = useState(false)
  const row = useRef<HTMLLIElement>(null)
  const wasConfirming = useRef(false)

  // A selection inside this project (for example a session just created) opens it.
  const hasSelected = sessions.some((s) => s.id === selectedId)
  useEffect(() => {
    if (hasSelected) setExpanded(true)
  }, [hasSelected])

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
          aria-expanded={expanded}
          onClick={() => setExpanded((v) => !v)}
          className="flex min-w-0 flex-1 items-center gap-1.5 rounded px-1 py-1.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          <ChevronRightIcon
            className={cn('size-3.5 shrink-0 text-muted transition-transform', expanded && 'rotate-90')}
            aria-hidden
          />
          <FolderIcon className="size-4 shrink-0 text-accent" aria-hidden />
          <span className="min-w-0 flex-1">
            <span className="flex items-center gap-1.5">
              <span className="truncate text-sm font-medium">{project.name}</span>
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
        <Count count={sessions.length} />
        <span className="flex shrink-0 items-center opacity-60 transition-opacity group-hover/row:opacity-100 group-focus-within/row:opacity-100">
          <NewSessionMenu
            project={project}
            agents={agents}
            disabled={project.missing || busy}
            onAgent={(agent) => onStart({ agent })}
            onCustom={() => setCustom(true)}
          />
          <IconAction label={`Remove ${project.name}`} data-row-remove destructive disabled={busy} onClick={() => setConfirming(true)}>
            <Trash2Icon />
          </IconAction>
        </span>
      </div>
      {custom && (
        <CustomCommandForm
          initial={defaultShell}
          onStart={(command) => onStart({ command }, () => setCustom(false))}
          onCancel={() => setCustom(false)}
        />
      )}
      {expanded && sessions.length > 0 && (
        <SessionItems sessions={sessions} labelOf={labelOf} selectedId={selectedId} onSelect={onSelect} />
      )}
    </li>
  )
}

/** The sidebar: every project as a parent node with its agent sessions nested beneath. */
export function ProjectTree() {
  const { data: projects, error: projectsError } = useProjectList()
  const { data: sessions = [], error: sessionsError } = useSessionList()
  const { data: metas = [] } = useSessionMetaList()
  const { data: agents = [] } = useAgentList()
  const { data: status } = useDaemonStatus()
  const selectedId = useSelectedId()
  const select = useUi((s) => s.select)
  const add = useAddProject()
  const remove = useRemoveProject()
  const create = useCreateSession()
  const [actionError, setActionError] = useState<string | null>(null)
  const addButton = useRef<HTMLButtonElement>(null)
  const inFlight = useRef(false) // `isPending` lags a render, so a fast double-click needs this

  const metaById = useMemo(() => new Map(metas.map((m) => [m.session_id, m])), [metas])
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

  function start(project: Project, target: Target, done?: () => void) {
    if (inFlight.current) return
    inFlight.current = true
    create.mutate(
      { ...target, projectId: project.id, cols: 80, rows: 24 },
      {
        onSuccess: (session) => {
          select(session.id)
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
      addButton.current?.focus() // the removed row had focus
    } catch (e) {
      setActionError(message(e))
    }
  }

  const busy = add.isPending || remove.isPending || create.isPending
  const defaultShell = status?.default_shell ?? ''
  const shared = { labelOf, selectedId, onSelect: select }

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
        {projectsError ? (
          <Alert variant="destructive" className="px-2.5 py-2">
            <AlertDescription>{projectsError.message}</AlertDescription>
          </Alert>
        ) : projects !== undefined && projects.length === 0 && sessions.length === 0 ? (
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
                  sessions={byProject[project.id] ?? []}
                  agents={agents}
                  defaultShell={defaultShell}
                  busy={busy}
                  onStart={(target, done) => start(project, target, done)}
                  onRemove={() => removeProject(project)}
                  {...shared}
                />
              ))}
              {unassigned.length > 0 && (
                <li>
                  <div className="flex items-center gap-1.5 px-2 pb-1 pt-2 text-[11px] font-semibold uppercase tracking-wider text-muted">
                    <span className="flex-1">Unassigned</span>
                    <Count count={unassigned.length} />
                  </div>
                  <SessionItems sessions={unassigned} {...shared} />
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
