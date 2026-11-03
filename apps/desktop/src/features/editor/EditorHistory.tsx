import { useQueryClient } from '@tanstack/react-query'
import { GitCommitHorizontalIcon, HistoryIcon } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'
import { queryKeys } from '../../queryClient'
import type { GitViewCommit } from '../../services/types'
import { ChangedFiles, DiffPatch, EmptyPane, ErrorPane, formatTime, LoadingPane, RefreshButton, SplitPane, StaleNotice } from './panes'
import { useGitDiff, useGitHistory } from './queries'

const ROW_H = 44
const LANE_W = 12
const GRAPH_PAD = 8

type LaneRow = { lane: number; before: (string | null)[]; after: (string | null)[]; parents: string[] }

// A lane holds the commit id expected next; parents fan out into free lanes.
function laneLayout(commits: GitViewCommit[]): LaneRow[] {
  const lanes: (string | null)[] = []
  return commits.map((commit) => {
    const before = [...lanes]
    let lane = lanes.indexOf(commit.id)
    if (lane < 0) {
      lane = lanes.indexOf(null)
      if (lane < 0) {
        lanes.push(commit.id)
        lane = lanes.length - 1
      } else {
        lanes[lane] = commit.id
      }
    }
    lanes[lane] = commit.parents[0] ?? null
    for (const parent of commit.parents.slice(1)) {
      if (lanes.includes(parent)) continue
      const free = lanes.indexOf(null)
      if (free < 0) lanes.push(parent)
      else lanes[free] = parent
    }
    while (lanes.length > 0 && lanes.at(-1) === null) lanes.pop()
    return { lane, before, after: [...lanes], parents: commit.parents }
  })
}

function Graph({ rows }: { rows: LaneRow[] }) {
  const lanes = Math.max(1, ...rows.map((row) => Math.max(row.lane + 1, row.after.length)))
  const width = GRAPH_PAD + lanes * LANE_W
  const height = rows.length * ROW_H
  const x = (lane: number) => GRAPH_PAD + lane * LANE_W + LANE_W / 2
  return (
    <svg width={width} height={height} className="shrink-0 text-accent" aria-hidden>
      {rows.map((row, i) => {
        const cy = i * ROW_H + ROW_H / 2
        const elements = []
        row.before.forEach((expected, lane) => {
          if (expected === null) return
          if (lane === row.lane) {
            elements.push(<line key={`in-${lane}`} x1={x(lane)} y1={i * ROW_H} x2={x(lane)} y2={cy} stroke="currentColor" strokeOpacity={0.6} />)
          } else {
            elements.push(<line key={`th-${lane}`} x1={x(lane)} y1={i * ROW_H} x2={x(lane)} y2={(i + 1) * ROW_H} stroke="currentColor" strokeOpacity={0.6} />)
          }
        })
        row.parents.forEach((parent, index) => {
          const lane = row.after.indexOf(parent)
          if (lane < 0) return
          elements.push(
            <line
              key={`out-${index}`}
              x1={x(row.lane)}
              y1={cy}
              x2={x(lane)}
              y2={Math.min(cy + ROW_H, height)}
              stroke="currentColor"
              strokeOpacity={0.6}
            />,
          )
        })
        elements.push(<circle key="dot" cx={x(row.lane)} cy={cy} r={3} fill="currentColor" />)
        return <g key={i}>{elements}</g>
      })}
    </svg>
  )
}

const refBadge = (ref: string) => {
  if (ref.startsWith('tag:')) return { label: ref.slice(4).trim(), variant: 'warning' as const }
  if (ref.startsWith('remotes/')) return { label: ref.slice(8), variant: 'outline' as const }
  return { label: ref, variant: 'secondary' as const }
}

export function EditorHistory({
  projectId,
  checkoutPath,
  active,
}: {
  projectId: string
  checkoutPath: string
  active: boolean
}) {
  const queryClient = useQueryClient()
  const history = useGitHistory(projectId, checkoutPath, active)
  const commits = useMemo(() => history.data?.pages.flatMap((page) => page.commits) ?? [], [history.data])
  const rows = useMemo(() => laneLayout(commits), [commits])
  const [selected, setSelected] = useState<string | null>(null)

  useEffect(() => {
    if (selected === null && commits.length > 0) {
      setSelected(commits.find((commit) => commit.head)?.id ?? commits[0].id)
    }
  }, [commits, selected])

  const commit = commits.find((item) => item.id === selected) ?? null
  const diff = useGitDiff(projectId, checkoutPath, selected ? { layer: 'commit', commit: selected } : null, { enabled: active })

  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: queryKeys.gitHistory(projectId, checkoutPath) })
    void queryClient.invalidateQueries({ queryKey: ['git-diff', projectId, checkoutPath] })
  }

  if (history.isPending) return <LoadingPane label="Loading history…" />
  if (history.isError && !history.data) return <ErrorPane error={history.error} onRetry={() => void history.refetch()} />

  if (commits.length === 0) {
    return (
      <EmptyPane icon={<HistoryIcon className="size-4" />} title="No commits yet">
        <p className="m-0">This repository has no commits on its local, remote-tracking, or tag refs.</p>
        <RefreshButton label="Refresh history" isFetching={history.isFetching} onClick={refresh} />
      </EmptyPane>
    )
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {history.isError && <StaleNotice error={history.error} />}
      <SplitPane
        aside={
          <>
            <header className="flex shrink-0 items-center justify-between border-b border-border px-2 py-1.5">
              <span className="text-[11px] font-semibold uppercase tracking-wider text-muted">History</span>
              <RefreshButton label="Refresh history" isFetching={history.isFetching} onClick={refresh} />
            </header>
            <ScrollArea className="min-h-0 flex-1">
              <div className="flex">
                <Graph rows={rows} />
                <ul className="m-0 min-w-0 flex-1 list-none p-0">
                  {commits.map((item) => (
                    <li key={item.id}>
                      <button
                        type="button"
                        aria-pressed={selected === item.id}
                        onClick={() => setSelected(item.id)}
                        className={cn(
                          'flex h-11 w-full flex-col justify-center gap-0.5 border-b border-border/50 px-2 text-left outline-none hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring',
                          selected === item.id && 'bg-secondary/70',
                        )}
                      >
                        <span className="flex min-w-0 items-center gap-1.5">
                          {item.head && (
                            <Badge variant="default" className="shrink-0 px-1 py-0 text-[10px]">HEAD</Badge>
                          )}
                          <span className="min-w-0 flex-1 truncate text-xs font-medium">{item.subject}</span>
                        </span>
                        <span className="flex min-w-0 items-center gap-1 text-[11px] text-muted">
                          <span className="shrink-0">{item.author}</span>
                          <span aria-hidden>·</span>
                          <span className="shrink-0">{formatTime(item.time)}</span>
                          {item.refs.map((ref) => {
                            const badge = refBadge(ref)
                            return (
                              <Badge key={ref} variant={badge.variant} className="max-w-28 truncate px-1 py-0 text-[10px]">
                                {badge.label}
                              </Badge>
                            )
                          })}
                        </span>
                      </button>
                    </li>
                  ))}
                  {history.hasNextPage && (
                    <li className="p-2">
                      <Button
                        variant="outline"
                        size="sm"
                        className="w-full"
                        disabled={history.isFetchingNextPage}
                        onClick={() => void history.fetchNextPage()}
                      >
                        {history.isFetchingNextPage ? 'Loading…' : 'Load more commits'}
                      </Button>
                    </li>
                  )}
                </ul>
              </div>
            </ScrollArea>
          </>
        }
      >
        {commit === null ? (
          <EmptyPane icon={<GitCommitHorizontalIcon className="size-4" />} title="Select a commit to see its patch" />
        ) : diff.isPending ? (
          <LoadingPane label="Loading commit…" />
        ) : diff.isError && !diff.data ? (
          <ErrorPane error={diff.error} onRetry={() => void diff.refetch()} />
        ) : diff.data ? (
          <>
            {diff.isError && <StaleNotice error={diff.error} />}
            <header className="flex shrink-0 flex-wrap items-center gap-x-2 gap-y-1 border-b border-border px-3 py-1.5">
              <span className="min-w-0 flex-1 truncate text-xs font-medium" title={commit.subject}>
                <span className="font-mono">{commit.id.slice(0, 8)}</span> {commit.subject}
              </span>
              <span className="shrink-0 text-xs text-muted">{commit.author} · {formatTime(commit.time)}</span>
            </header>
            <ChangedFiles files={diff.data.files} />
            <DiffPatch patch={diff.data.patch} truncated={diff.data.truncated} />
          </>
        ) : null}
      </SplitPane>
    </div>
  )
}
