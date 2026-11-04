import { AlertTriangleIcon, FolderGit2Icon } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import type { GitDiffRequest } from '../../services/git-view'
import type { Project } from '../../services/types'
import { useUi } from '../../stores/ui'
import { useWorktreeList } from '../worktrees/queries'
import { EditorChanges } from './EditorChanges'
import { EditorFiles } from './EditorFiles'
import { EditorHistory } from './EditorHistory'
import { EditorStashes } from './EditorStashes'
import { EmptyPane, ErrorPane, LoadingPane, StaleNotice } from './panes'
import { ViewSwitch } from './ViewSwitch'

type Section = 'files' | 'changes' | 'history' | 'stashes'
const SECTIONS: { id: Section; label: string }[] = [
  { id: 'files', label: 'Files' },
  { id: 'changes', label: 'Changes' },
  { id: 'history', label: 'History' },
  { id: 'stashes', label: 'Stashes' },
]

/** The workspace-level Editor: Git inspection and file editing for the active checkout. */
export function EditorWorkspace({
  project,
  projectId,
  checkoutPath,
}: {
  project: Project | null
  projectId: string
  checkoutPath: string
}) {
  const setActiveView = useUi((state) => state.setActiveView)
  const worktreeQuery = useWorktreeList(projectId, Boolean(project))
  const [section, setSection] = useState<Section>('changes')
  const [jump, setJump] = useState<GitDiffRequest | null>(null)
  const root = useRef<HTMLElement | null>(null)
  // Once visited, a section stays mounted so its state and scroll survive;
  // its queries only run while it is the visible section.
  const [visited, setVisited] = useState<ReadonlySet<Section>>(new Set(['changes']))
  useEffect(() => {
    setVisited((current) => (current.has(section) ? current : new Set(current).add(section)))
  }, [section])

  const repo = worktreeQuery.data?.repo
  const checkout = worktreeQuery.data?.worktrees.find((item) => item.path === checkoutPath)
  const isRoot = project !== null && project.root === checkoutPath
  const unsupported = project === null || repo === false
  const missingCheckout =
    !unsupported &&
    (project?.missing === true || checkout?.missing === true || (repo === true && !isRoot && checkout === undefined))
  const ready = !unsupported && !missingCheckout && repo === true

  useEffect(() => {
    if (!ready) return
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement
      const typing =
        target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {
        const state = useUi.getState()
        if (state.editorDraft && state.editorSaveDraft) {
          event.preventDefault()
          void state.editorSaveDraft()
        }
        return
      }
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'f') {
        if (target instanceof Element && target.closest('.cm-editor')) return
        const input = root.current?.querySelector<HTMLInputElement>(
          `[data-section="${section}"] input[data-editor-filter]`,
        )
        if (input) {
          event.preventDefault()
          input.focus()
          input.select()
        }
        return
      }
      if (typing || event.metaKey || event.ctrlKey || event.altKey) return
      const index = Number(event.key) - 1
      if (index >= 0 && index < SECTIONS.length) {
        event.preventDefault()
        setSection(SECTIONS[index].id)
        return
      }
      if (event.key === 'j' || event.key === 'k') {
        const items = Array.from(
          root.current?.querySelectorAll<HTMLElement>(`[data-section="${section}"] [data-nav-item]`) ?? [],
        )
        if (items.length === 0) return
        event.preventDefault()
        const current = items.indexOf(document.activeElement as HTMLElement)
        const next =
          event.key === 'j'
            ? current < 0
              ? 0
              : Math.min(items.length - 1, current + 1)
            : current < 0
              ? items.length - 1
              : Math.max(0, current - 1)
        items[next].focus()
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [ready, section])

  const branch = checkout?.branch ?? (checkout ? `detached ${checkout.head.slice(0, 8)}` : null)
  const checkoutLabel = checkout?.name ?? checkoutPath.split('/').filter(Boolean).at(-1) ?? checkoutPath
  const title = project?.name ?? 'Editor'

  const showChanges = (request: GitDiffRequest) => {
    setJump(request)
    setSection('changes')
  }

  const body = () => {
    if (unsupported) {
      return (
        <EmptyPane icon={<FolderGit2Icon className="size-4" />} title="Editor needs a Git checkout">
          <p className="m-0">
            {project === null
              ? 'This workspace is not a registered project checkout.'
              : `${project.name} is not a Git repository, so there is nothing to inspect.`}
          </p>
          <Button variant="outline" size="sm" onClick={() => setActiveView('terminal')}>
            Back to Terminal
          </Button>
        </EmptyPane>
      )
    }
    if (missingCheckout) {
      return (
        <EmptyPane icon={<AlertTriangleIcon className="size-4" />} title="This checkout is missing">
          <p className="m-0">Restore the checkout directory or refresh the worktree list, then try again.</p>
          <Button variant="outline" size="sm" onClick={() => setActiveView('terminal')}>
            Back to Terminal
          </Button>
        </EmptyPane>
      )
    }
    if (worktreeQuery.isPending) return <LoadingPane label="Loading Git data…" />
    if (worktreeQuery.isError && !worktreeQuery.data) {
      return <ErrorPane error={worktreeQuery.error} onRetry={() => void worktreeQuery.refetch()} />
    }
    if (!ready) return <LoadingPane label="Loading Git data…" />
    return (
      <>
        {worktreeQuery.isError && worktreeQuery.data && <StaleNotice error={worktreeQuery.error} />}
        {SECTIONS.map(
          (item) =>
            (visited.has(item.id) || item.id === section) && (
              <div key={item.id} data-section={item.id} hidden={item.id !== section} className="flex min-h-0 flex-1 flex-col">
                {item.id === 'files' && (
                  <EditorFiles
                    key={checkoutPath}
                    projectId={projectId}
                    checkoutPath={checkoutPath}
                    active={section === 'files'}
                    onShowChanges={showChanges}
                  />
                )}
                {item.id === 'changes' && (
                  <EditorChanges
                    key={checkoutPath}
                    projectId={projectId}
                    checkoutPath={checkoutPath}
                    active={section === 'changes'}
                    jump={jump}
                    onJumpHandled={() => setJump(null)}
                  />
                )}
                {item.id === 'history' && (
                  <EditorHistory
                    key={checkoutPath}
                    projectId={projectId}
                    checkoutPath={checkoutPath}
                    active={section === 'history'}
                  />
                )}
                {item.id === 'stashes' && (
                  <EditorStashes
                    key={projectId}
                    projectId={projectId}
                    checkoutPath={checkoutPath}
                    active={section === 'stashes'}
                  />
                )}
              </div>
            ),
        )}
      </>
    )
  }

  return (
    <section ref={root} aria-label={`${title} editor`} className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex min-h-10 shrink-0 items-center gap-2 border-b border-border bg-panel px-2">
        <ViewSwitch view="editor" onChange={setActiveView} />
        <span className="truncate text-sm font-medium">{title}</span>
        <Badge variant="secondary" title={checkoutPath}>{checkoutLabel}</Badge>
        <span className="min-w-0 truncate text-xs text-muted" title={checkoutPath}>{checkoutPath}</span>
        {branch && <Badge variant="outline">{branch}</Badge>}
      </div>
      <div role="tablist" aria-label="Editor sections" className="flex shrink-0 items-center gap-1 border-b border-border bg-panel px-2">
        {SECTIONS.map((item) => (
          <button
            key={item.id}
            type="button"
            role="tab"
            aria-selected={section === item.id}
            onClick={() => setSection(item.id)}
            className={cn(
              'h-9 border-b-2 px-2.5 text-xs outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring',
              section === item.id ? 'border-accent font-medium text-text' : 'border-transparent text-muted hover:text-text',
            )}
          >
            {item.label}
          </button>
        ))}
      </div>
      {body()}
    </section>
  )
}
