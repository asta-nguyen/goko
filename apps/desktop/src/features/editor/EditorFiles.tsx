import { useQueryClient } from '@tanstack/react-query'
import { ChevronRightIcon, FileIcon, FileWarningIcon, FolderIcon } from 'lucide-react'
import { useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'
import { queryKeys } from '../../queryClient'
import type { GitDiffRequest } from '../../services/git-view'
import type { GitViewDiffLayer } from '../../services/types'
import { CodeView, EmptyPane, ErrorPane, LoadingPane, RefreshButton, SplitPane, StaleNotice } from './panes'
import { useGitChanges, useGitFile, useGitTree } from './queries'

const sizeLabel = (size: number) => (size >= 1024 * 1024 ? `${(size / 1024 / 1024).toFixed(1)} MiB` : `${Math.max(1, Math.round(size / 1024))} KiB`)

function TreeRows({
  projectId,
  checkoutPath,
  directory,
  depth,
  expanded,
  onToggle,
  selected,
  onSelect,
  active,
}: {
  projectId: string
  checkoutPath: string
  directory: string
  depth: number
  expanded: Record<string, boolean>
  onToggle: (directory: string) => void
  selected: string | null
  onSelect: (path: string) => void
  active: boolean
}) {
  const tree = useGitTree(projectId, checkoutPath, directory, active)
  const indent = { paddingLeft: depth * 12 + 4 }

  if (tree.isPending) return <p className="m-0 py-1 text-xs text-muted" style={indent}>Loading…</p>
  if (tree.isError && !tree.data) {
    return (
      <p role="alert" className="m-0 flex items-center gap-1 py-1 text-xs text-danger" style={indent}>
        Failed to load.
        <Button variant="ghost" size="xs" onClick={() => void tree.refetch()}>Retry</Button>
      </p>
    )
  }
  const entries = tree.data?.entries ?? []
  const stale = tree.isError && tree.data ? <StaleNotice error={tree.error} /> : null
  if (!entries.length) return <>{stale}<p className="m-0 py-1 text-xs text-muted" style={indent}>Empty</p></>

  return (
    <>
      {stale}
      <ul className="m-0 list-none p-0">
        {entries.map((entry) => {
          const isOpen = expanded[entry.path] === true
          if (entry.kind === 'dir') {
            return (
              <li key={entry.path}>
                <button
                  type="button"
                  aria-expanded={isOpen}
                  onClick={() => onToggle(entry.path)}
                  style={indent}
                  className="flex w-full items-center gap-1 rounded px-1 py-1 text-left outline-none hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-ring"
                >
                  <ChevronRightIcon className={cn('size-3.5 shrink-0 text-muted transition-transform', isOpen && 'rotate-90')} aria-hidden />
                  <FolderIcon className="size-3.5 shrink-0 text-accent" aria-hidden />
                  <span className="truncate text-xs">{entry.name}</span>
                </button>
                {isOpen && (
                  <TreeRows
                    projectId={projectId}
                    checkoutPath={checkoutPath}
                    directory={entry.path}
                    depth={depth + 1}
                    expanded={expanded}
                    onToggle={onToggle}
                    selected={selected}
                    onSelect={onSelect}
                    active={active}
                  />
                )}
              </li>
            )
          }
          return (
            <li key={entry.path}>
              <button
                type="button"
                aria-pressed={selected === entry.path}
                onClick={() => onSelect(entry.path)}
                style={{ paddingLeft: depth * 12 + 18 }}
                className={cn(
                  'flex w-full items-center gap-1.5 rounded px-1 py-1 text-left outline-none hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-ring',
                  selected === entry.path && 'bg-secondary/70',
                )}
              >
                <FileIcon className="size-3.5 shrink-0 text-muted" aria-hidden />
                <span className="truncate font-mono text-xs" title={entry.path}>{entry.name}</span>
              </button>
            </li>
          )
        })}
      </ul>
    </>
  )
}

export function EditorFiles({
  projectId,
  checkoutPath,
  active,
  onShowChanges,
}: {
  projectId: string
  checkoutPath: string
  active: boolean
  onShowChanges: (request: GitDiffRequest) => void
}) {
  const queryClient = useQueryClient()
  const [expanded, setExpanded] = useState<Record<string, boolean>>({})
  const [path, setPath] = useState<string | null>(null)
  const file = useGitFile(projectId, checkoutPath, path, active)
  // No polling here — one fresh status read powers the "show changes" link.
  const changes = useGitChanges(projectId, checkoutPath, active, false)

  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: ['git-tree', projectId, checkoutPath] })
    if (path !== null) void queryClient.invalidateQueries({ queryKey: queryKeys.gitFile(projectId, checkoutPath, path) })
  }

  const layerFor = (target: string): GitViewDiffLayer | null => {
    const data = changes.data
    if (!data) return null
    if (data.conflicts.some((entry) => entry.path === target)) return 'conflict'
    if (data.staged.some((entry) => entry.path === target)) return 'staged'
    if (data.unstaged.some((entry) => entry.path === target)) return 'unstaged'
    if (data.untracked.some((entry) => entry.path === target)) return 'untracked'
    return null
  }
  const changedLayer = path !== null ? layerFor(path) : null

  return (
    <SplitPane
      aside={
        <>
          <header className="flex shrink-0 items-center justify-between border-b border-border px-2 py-1.5">
            <span className="text-[11px] font-semibold uppercase tracking-wider text-muted">Files</span>
            <RefreshButton label="Refresh file tree" isFetching={file.isFetching} onClick={refresh} />
          </header>
          <ScrollArea className="min-h-0 flex-1">
            <div className="p-1.5">
              <TreeRows
                projectId={projectId}
                checkoutPath={checkoutPath}
                directory=""
                depth={0}
                expanded={expanded}
                onToggle={(directory) => setExpanded((current) => ({ ...current, [directory]: !current[directory] }))}
                selected={path}
                onSelect={setPath}
                active={active}
              />
            </div>
          </ScrollArea>
        </>
      }
    >
      {path === null ? (
        <EmptyPane icon={<FileIcon className="size-4" />} title="Select a file to preview it" />
      ) : file.isPending ? (
        <LoadingPane label="Loading file…" />
      ) : file.isError && !file.data ? (
        <ErrorPane error={file.error} onRetry={() => void file.refetch()} />
      ) : file.data ? (
        <>
          {file.isError && <StaleNotice error={file.error} />}
          <header className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border px-3 py-1.5">
            <span className="min-w-0 flex-1 truncate font-mono text-xs font-medium" title={file.data.path}>{file.data.path}</span>
            <Badge variant="secondary">{sizeLabel(file.data.size)}</Badge>
            {file.data.truncated && <Badge variant="warning">truncated</Badge>}
            {changedLayer && (
              <Button variant="outline" size="sm" onClick={() => onShowChanges({ layer: changedLayer, path })}>
                Show changes
              </Button>
            )}
          </header>
          {file.data.kind === 'text' && file.data.text !== null ? (
            <CodeView text={file.data.text} />
          ) : (
            <EmptyPane icon={<FileWarningIcon className="size-4" />} title={
              file.data.kind === 'binary'
                ? 'Binary file — no text preview'
                : file.data.kind === 'nonutf8'
                  ? 'Not valid UTF-8 — no text preview'
                  : file.data.kind === 'missing'
                    ? 'This file is missing'
                    : 'This file cannot be read'
            }>
              <p className="m-0">{sizeLabel(file.data.size)}</p>
            </EmptyPane>
          )}
        </>
      ) : null}
    </SplitPane>
  )
}
