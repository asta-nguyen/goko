import { useQueryClient } from '@tanstack/react-query'
import { CheckIcon, ChevronRightIcon, FileIcon } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'
import { queryKeys } from '../../queryClient'
import type { GitDiffRequest } from '../../services/git-view'
import type { GitViewChanges, GitViewDiffLayer, GitViewStatusEntry } from '../../services/types'
import { DiffPatch, DiffViewToggle, EmptyPane, entryName, ErrorPane, FilterInput, LoadingPane, matchesQuery, RefreshButton, SplitPane, StaleNotice, statusLabel } from './panes'
import { useGitChanges, useGitDiff } from './queries'

type Layer = Extract<GitViewDiffLayer, 'conflict' | 'staged' | 'unstaged' | 'untracked'>
type Selection = { layer: Layer; entry: GitViewStatusEntry }

const GROUPS: { key: keyof Pick<GitViewChanges, 'conflicts' | 'staged' | 'unstaged' | 'untracked'>; layer: Layer; label: string }[] = [
  { key: 'conflicts', layer: 'conflict', label: 'Conflicts' },
  { key: 'staged', layer: 'staged', label: 'Staged' },
  { key: 'unstaged', layer: 'unstaged', label: 'Unstaged' },
  { key: 'untracked', layer: 'untracked', label: 'Untracked' },
]

function firstChange(data: GitViewChanges | undefined): Selection | null {
  if (!data) return null
  for (const group of GROUPS) {
    const entry = data[group.key][0]
    if (entry) return { layer: group.layer, entry }
  }
  return null
}

function findEntry(data: GitViewChanges | undefined, layer: Layer, path: string): GitViewStatusEntry | null {
  if (!data) return null
  const group = GROUPS.find((item) => item.layer === layer)
  return group ? (data[group.key].find((entry) => entry.path === path) ?? null) : null
}

export function EditorChanges({
  projectId,
  checkoutPath,
  active,
  jump,
  onJumpHandled,
}: {
  projectId: string
  checkoutPath: string
  active: boolean
  jump: GitDiffRequest | null
  onJumpHandled: () => void
}) {
  const queryClient = useQueryClient()
  const changes = useGitChanges(projectId, checkoutPath, active)
  const [selected, setSelected] = useState<Selection | null>(null)
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({})
  const [filter, setFilter] = useState('')
  const first = firstChange(changes.data)

  // A Files→Changes jump wins over the default first-change selection.
  useEffect(() => {
    if (!jump || !jump.path) return
    const entry = findEntry(changes.data, jump.layer as Layer, jump.path) ?? {
      path: jump.path,
      orig_path: null,
      status: 'modified' as const,
    }
    setSelected({ layer: jump.layer as Layer, entry })
    onJumpHandled()
  }, [jump, changes.data, onJumpHandled])
  useEffect(() => {
    if (selected === null && first) setSelected(first)
  }, [first, selected])

  const request = useMemo<GitDiffRequest | null>(
    () => (selected ? { layer: selected.layer, path: selected.entry.path } : null),
    [selected],
  )
  const diff = useGitDiff(projectId, checkoutPath, request, { enabled: active, poll: true })

  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: queryKeys.gitChanges(projectId, checkoutPath) })
    void queryClient.invalidateQueries({ queryKey: ['git-diff', projectId, checkoutPath] })
  }

  if (changes.isPending) return <LoadingPane label="Loading changes…" />
  if (changes.isError && !changes.data) return <ErrorPane error={changes.error} onRetry={() => void changes.refetch()} />

  const data = changes.data
  const isClean = data ? GROUPS.every((group) => data[group.key].length === 0) : false

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {changes.isError && <StaleNotice error={changes.error} />}
      {isClean ? (
        <EmptyPane icon={<CheckIcon className="size-4" />} title="Working tree clean">
          <p className="m-0">No staged, unstaged, or untracked changes.</p>
          <RefreshButton label="Refresh changes" isFetching={changes.isFetching} onClick={refresh} />
        </EmptyPane>
      ) : (
        <SplitPane
          aside={
            <>
              <header className="flex shrink-0 items-center justify-between border-b border-border px-2 py-1.5">
                <span className="text-[11px] font-semibold uppercase tracking-wider text-muted">Changed files</span>
                <RefreshButton label="Refresh changes" isFetching={changes.isFetching || diff.isFetching} onClick={refresh} />
              </header>
              <div className="shrink-0 border-b border-border px-2 py-1">
                <FilterInput label="Filter changed files" value={filter} onChange={setFilter} />
              </div>
              <ScrollArea className="min-h-0 flex-1">
                <div className="flex flex-col gap-0.5 p-1.5">
                  {GROUPS.map((group) => {
                    const entries = (data?.[group.key] ?? []).filter((entry) => matchesQuery(entryName(entry), filter))
                    const isCollapsed = collapsed[group.key] === true
                    return (
                      <section key={group.key} aria-label={`${group.label} files`}>
                        <button
                          type="button"
                          aria-expanded={!isCollapsed}
                          onClick={() => setCollapsed((current) => ({ ...current, [group.key]: !isCollapsed }))}
                          className="flex w-full items-center gap-1 rounded px-1 py-1 text-left outline-none hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-ring"
                        >
                          <ChevronRightIcon
                            className={cn('size-3.5 shrink-0 text-muted transition-transform', !isCollapsed && 'rotate-90')}
                            aria-hidden
                          />
                          <span className="flex-1 text-xs font-medium">{group.label}</span>
                          <Badge variant="secondary" aria-label={`${entries.length} ${group.label.toLowerCase()}`} className="tabular-nums">
                            {entries.length}
                          </Badge>
                        </button>
                        {!isCollapsed && (
                          <ul className="m-0 list-none p-0 pl-4">
                            {entries.length === 0 && <li className="px-1 py-0.5 text-xs text-muted">None</li>}
                            {entries.map((entry) => {
                              const isSelected = selected?.layer === group.layer && selected.entry.path === entry.path
                              return (
                                <li key={`${group.layer}:${entry.path}`}>
                                  <button
                                    type="button"
                                    data-nav-item
                                    aria-pressed={isSelected}
                                    onClick={() => setSelected({ layer: group.layer, entry })}
                                    className={cn(
                                      'flex w-full items-center gap-1.5 rounded px-1.5 py-1 text-left outline-none hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-ring',
                                      isSelected && 'bg-secondary/70',
                                    )}
                                  >
                                    <FileIcon className="size-3.5 shrink-0 text-muted" aria-hidden />
                                    <span className="min-w-0 flex-1 truncate font-mono text-xs" title={entryName(entry)}>
                                      {entry.orig_path && <span className="text-muted">{entry.orig_path} → </span>}
                                      {entry.path}
                                    </span>
                                    <Badge variant="outline" className="shrink-0">{statusLabel(entry.status)}</Badge>
                                  </button>
                                </li>
                              )
                            })}
                          </ul>
                        )}
                      </section>
                    )
                  })}
                </div>
              </ScrollArea>
            </>
          }
        >
          {selected === null ? (
            <EmptyPane icon={<FileIcon className="size-4" />} title="Select a changed file to see its diff" />
          ) : diff.isPending ? (
            <LoadingPane label="Loading diff…" />
          ) : diff.isError && !diff.data ? (
            <ErrorPane error={diff.error} onRetry={() => void diff.refetch()} />
          ) : diff.data ? (
            <>
              {diff.isError && <StaleNotice error={diff.error} />}
              <header className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border px-3 py-1.5">
                <span className="min-w-0 truncate font-mono text-xs font-medium" title={entryName(selected.entry)}>
                  {entryName(selected.entry)}
                </span>
                <Badge variant="secondary">{diff.data.label}</Badge>
                <DiffViewToggle />
              </header>
              <DiffPatch patch={diff.data.patch} truncated={diff.data.truncated} />
            </>
          ) : null}
        </SplitPane>
      )}
    </div>
  )
}
