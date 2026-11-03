import { useQueryClient } from '@tanstack/react-query'
import { ArchiveIcon, ArchiveRestoreIcon } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'
import { queryKeys } from '../../queryClient'
import { ChangedFiles, DiffPatch, EmptyPane, ErrorPane, formatTime, LoadingPane, RefreshButton, SplitPane, StaleNotice } from './panes'
import { useGitStashes, useGitStashDiff } from './queries'

export function EditorStashes({
  projectId,
  checkoutPath,
  active,
}: {
  projectId: string
  checkoutPath: string
  active: boolean
}) {
  const queryClient = useQueryClient()
  const stashes = useGitStashes(projectId, checkoutPath, active)
  const [selected, setSelected] = useState<string | null>(null)

  // The stack is repository-wide: the newest entry is the default selection.
  useEffect(() => {
    if (selected === null && stashes.data?.length) setSelected(stashes.data[0].id)
  }, [stashes.data, selected])

  const stash = stashes.data?.find((item) => item.id === selected) ?? null
  const diff = useGitStashDiff(projectId, checkoutPath, selected, active)

  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: queryKeys.gitStashes(projectId) })
    if (selected !== null) void queryClient.invalidateQueries({ queryKey: queryKeys.gitStashDiff(projectId, selected) })
  }

  if (stashes.isPending) return <LoadingPane label="Loading stashes…" />
  if (stashes.isError && !stashes.data) return <ErrorPane error={stashes.error} onRetry={() => void stashes.refetch()} />

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {stashes.isError && <StaleNotice error={stashes.error} />}
      <SplitPane
        aside={
          <>
            <header className="flex shrink-0 items-center justify-between border-b border-border px-2 py-1.5">
              <span className="text-[11px] font-semibold uppercase tracking-wider text-muted">Stashes</span>
              <RefreshButton label="Refresh stashes" isFetching={stashes.isFetching || diff.isFetching} onClick={refresh} />
            </header>
            <p className="m-0 shrink-0 border-b border-border px-3 py-1.5 text-[11px] text-muted">
              Shared across all worktrees of this repository.
            </p>
            <ScrollArea className="min-h-0 flex-1">
              {stashes.data?.length === 0 ? (
                <p className="m-0 p-3 text-xs text-muted">No stashes in this repository.</p>
              ) : (
                <ul className="m-0 flex list-none flex-col gap-0.5 p-1.5">
                  {stashes.data?.map((item) => (
                    <li key={item.id}>
                      <button
                        type="button"
                        aria-pressed={selected === item.id}
                        onClick={() => setSelected(item.id)}
                        className={cn(
                          'flex w-full flex-col gap-0.5 rounded px-2 py-1.5 text-left outline-none hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-ring',
                          selected === item.id && 'bg-secondary/70',
                        )}
                      >
                        <span className="flex min-w-0 items-center gap-1.5">
                          <span className="shrink-0 font-mono text-xs font-medium">{`stash@{${item.index}}`}</span>
                          {item.branch && <Badge variant="outline" className="shrink-0">{item.branch}</Badge>}
                          <span className="shrink-0 text-[11px] text-muted">{formatTime(item.time)}</span>
                        </span>
                        <span className="truncate text-xs text-muted" title={item.message}>{item.message}</span>
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </ScrollArea>
          </>
        }
      >
        {selected === null ? (
          <EmptyPane icon={<ArchiveIcon className="size-4" />} title="Select a stash to see its patch">
            <p className="m-0">The stash stack is shared by every worktree of this repository.</p>
          </EmptyPane>
        ) : diff.isPending ? (
          <LoadingPane label="Loading stash…" />
        ) : diff.isError && !diff.data ? (
          <ErrorPane error={diff.error} onRetry={() => void diff.refetch()} />
        ) : diff.data ? (
          <>
            {diff.isError && <StaleNotice error={diff.error} />}
            <header className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border px-3 py-1.5">
              <ArchiveRestoreIcon className="size-3.5 shrink-0 text-muted" aria-hidden />
              <span className="min-w-0 flex-1 truncate text-xs font-medium" title={stash?.message}>
                <span className="font-mono">{`stash@{${stash?.index ?? '?'}}`}</span>
                {stash?.message ? ` ${stash.message}` : ''}
              </span>
              {stash?.branch && <Badge variant="secondary">branch: {stash.branch}</Badge>}
              {stash && <span className="shrink-0 text-xs text-muted">{formatTime(stash.time)}</span>}
            </header>
            <ChangedFiles files={diff.data.files} />
            <DiffPatch patch={diff.data.patch} truncated={diff.data.truncated} />
          </>
        ) : null}
      </SplitPane>
    </div>
  )
}
