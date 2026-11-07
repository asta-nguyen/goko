import { useEffect, useImperativeHandle, useMemo, useRef } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from '@/components/ui/context-menu'
import { ResizableHandle } from '@/components/ui/resizable-handle'
import { cn } from '@/lib/utils'
import type { GitViewCommit } from '../../services/types'
import { GRAPH_PALETTE, layoutGraph } from './history-graph'
import { LAYOUT_LIMITS, type HistoryColumn, type HistoryLayout } from './history-layout'
import { formatTime, refBadge } from './panes'

const ROW_H = 28
const LANE_W = 14
const GRAPH_PAD = 8
const DESCRIPTION_FLOOR = 200

export interface HistoryTableHandle {
  scrollTo(id: string): void
}

const relative = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' })
const MINUTE = 60_000
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR
const MONTH = 30 * DAY

/** Relative commit age; switches to an absolute medium date past 30 days. */
export function formatRelative(epochMs: number, now = Date.now()): string {
  const elapsed = now - epochMs
  if (elapsed < MINUTE) return 'just now'
  if (elapsed < MONTH) {
    if (elapsed < HOUR) return relative.format(-Math.round(elapsed / MINUTE), 'minute')
    if (elapsed < DAY) return relative.format(-Math.round(elapsed / HOUR), 'hour')
    return relative.format(-Math.round(elapsed / DAY), 'day')
  }
  return new Date(epochMs).toLocaleDateString(undefined, { dateStyle: 'medium' })
}

const copy = (text: string) => {
  void navigator.clipboard?.writeText(text)?.catch(() => {})
}

/** The History commit table: lane graph, resizable columns, rows, and paging. */
export function HistoryTable({
  commits,
  selected,
  onSelect,
  matches,
  hasNextPage,
  isFetchingNextPage,
  onLoadMore,
  uncommitted,
  onSelectUncommitted,
  layout,
  onColumn,
  onResetColumn,
  ref,
}: {
  commits: GitViewCommit[]
  selected: string | null
  onSelect: (id: string) => void
  /** Ids matching the container's filter; `null` means no filter is active. */
  matches: Set<string> | null
  hasNextPage: boolean
  isFetchingNextPage: boolean
  onLoadMore: () => void
  uncommitted: { count: number; selected: boolean } | null
  onSelectUncommitted: () => void
  /** Column widths owned by the container's `useHistoryLayout`. */
  layout: HistoryLayout
  onColumn: (name: HistoryColumn, px: number) => void
  onResetColumn: (name: HistoryColumn) => void
  ref?: React.Ref<HistoryTableHandle>
}) {
  const { rows, lanes } = useMemo(() => layoutGraph(commits), [commits])
  const naturalGraph = Math.min(
    LAYOUT_LIMITS.graph.max,
    Math.max(LAYOUT_LIMITS.graph.min, lanes * LANE_W + GRAPH_PAD),
  )
  const graphWidth = layout.graph ?? naturalGraph
  const dateWidth = layout.date
  const authorWidth = layout.author
  const commitWidth = layout.commit
  const minWidth = graphWidth + dateWidth + authorWidth + commitWidth + DESCRIPTION_FLOOR

  const rowOffset = uncommitted ? ROW_H : 0
  const headIndex = rows.findIndex((row) => row.head)
  const headRow = headIndex >= 0 ? rows[headIndex] : null
  const laneX = (lane: number) => GRAPH_PAD + lane * LANE_W + LANE_W / 2
  const rowTop = (index: number) => rowOffset + index * ROW_H
  const rowCy = (index: number) => rowTop(index) + ROW_H / 2
  const rowBottom = (index: number) => rowTop(index) + ROW_H
  const svgHeight = rowOffset + rows.length * ROW_H

  const sentinelRef = useRef<HTMLLIElement>(null)
  const rowRefs = useRef(new Map<string, HTMLElement>())
  const loadMore = useRef({ fetch: onLoadMore, busy: isFetchingNextPage })

  useEffect(() => {
    loadMore.current = { fetch: onLoadMore, busy: isFetchingNextPage }
  })

  useEffect(() => {
    const element = sentinelRef.current
    if (!element || typeof IntersectionObserver === 'undefined') return
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting) && !loadMore.current.busy) {
        loadMore.current.fetch()
      }
    })
    observer.observe(element)
    return () => observer.disconnect()
    // Observing again after each page makes the observer report a sentinel that is still in view.
  }, [hasNextPage, commits.length])

  useImperativeHandle(ref, () => ({
    scrollTo: (id: string) => {
      rowRefs.current.get(id)?.scrollIntoView?.({ block: 'nearest' })
    },
  }))

  const onListKeyDown = (event: React.KeyboardEvent<HTMLUListElement>) => {
    const items = Array.from(event.currentTarget.querySelectorAll<HTMLElement>('[data-nav-item]'))
    const index = items.indexOf(document.activeElement as HTMLElement)
    let next: HTMLElement | undefined
    if (event.key === 'ArrowDown') next = items[index + 1]
    else if (event.key === 'ArrowUp') next = items[index - 1]
    else if (event.key === 'Home') next = items[0]
    else if (event.key === 'End') next = items.at(-1)
    else return
    event.preventDefault()
    next?.focus()
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-auto">
        <div style={{ minWidth }}>
          <div className="sticky top-0 z-10 flex h-7 items-stretch border-b border-border bg-bg text-[11px] font-semibold uppercase tracking-wider text-muted">
            <span className="flex shrink-0 items-center truncate px-2" style={{ width: graphWidth }}>
              Graph
            </span>
            <ResizableHandle
              orientation="vertical"
              label="Resize graph column"
              value={graphWidth}
              min={LAYOUT_LIMITS.graph.min}
              max={LAYOUT_LIMITS.graph.max}
              onChange={(px) => onColumn('graph', px)}
              onReset={() => onResetColumn('graph')}
            />
            <span className="flex min-w-0 flex-1 items-center gap-2 px-2">
              <span className="truncate">Description</span>
              {matches !== null && (
                <span className="ml-auto shrink-0 font-normal normal-case">{matches.size} {matches.size === 1 ? 'match' : 'matches'}</span>
              )}
            </span>
            <ResizableHandle
              orientation="vertical"
              label="Resize date column"
              value={dateWidth}
              min={LAYOUT_LIMITS.date.min}
              max={LAYOUT_LIMITS.date.max}
              invert
              onChange={(px) => onColumn('date', px)}
              onReset={() => onResetColumn('date')}
            />
            <span className="flex shrink-0 items-center truncate px-2" style={{ width: dateWidth }}>
              Date
            </span>
            <ResizableHandle
              orientation="vertical"
              label="Resize author column"
              value={authorWidth}
              min={LAYOUT_LIMITS.author.min}
              max={LAYOUT_LIMITS.author.max}
              invert
              onChange={(px) => onColumn('author', px)}
              onReset={() => onResetColumn('author')}
            />
            <span className="flex shrink-0 items-center truncate px-2" style={{ width: authorWidth }}>
              Author
            </span>
            <ResizableHandle
              orientation="vertical"
              label="Resize commit column"
              value={commitWidth}
              min={LAYOUT_LIMITS.commit.min}
              max={LAYOUT_LIMITS.commit.max}
              invert
              onChange={(px) => onColumn('commit', px)}
              onReset={() => onResetColumn('commit')}
            />
            <span className="flex shrink-0 items-center truncate px-2" style={{ width: commitWidth }}>
              Commit
            </span>
          </div>
          <div className="relative">
            <ul className="m-0 list-none p-0" onKeyDown={onListKeyDown}>
              {uncommitted && (
                <li>
                  <button
                    type="button"
                    data-nav-item
                    aria-pressed={uncommitted.selected}
                    onClick={onSelectUncommitted}
                    className={cn(
                      'flex h-7 w-full items-center border-b border-border/50 text-left text-xs text-muted outline-none hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring',
                      uncommitted.selected && 'bg-secondary/70',
                    )}
                  >
                    <span className="shrink-0" style={{ width: graphWidth }} aria-hidden />
                    <span className="w-2 shrink-0" aria-hidden />
                    <span className="min-w-0 flex-1 truncate px-2">
                      Uncommitted changes ({uncommitted.count})
                    </span>
                  </button>
                </li>
              )}
              {commits.map((commit, index) => {
                const row = rows[index]
                const paint = GRAPH_PALETTE[row.color]
                return (
                  <li key={commit.id}>
                    <ContextMenu>
                      <ContextMenuTrigger asChild>
                        <button
                          type="button"
                          data-nav-item
                          aria-pressed={selected === commit.id}
                          ref={(element) => {
                            if (element) rowRefs.current.set(commit.id, element)
                            else rowRefs.current.delete(commit.id)
                          }}
                          onClick={() => onSelect(commit.id)}
                          className={cn(
                            'flex h-7 w-full items-center border-b border-border/50 text-left outline-none hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring',
                            selected === commit.id && 'bg-secondary/70',
                            matches !== null && !matches.has(commit.id) && 'opacity-40',
                          )}
                        >
                          <span className="shrink-0" style={{ width: graphWidth }} aria-hidden />
                          <span className="w-2 shrink-0" aria-hidden />
                          <span className="flex min-w-0 flex-1 items-center gap-1 overflow-hidden px-2">
                            <span className="min-w-24 flex-1 truncate text-xs" title={commit.subject}>
                              {commit.subject}
                            </span>
                            {(commit.head || commit.refs.length > 0) && (
                              <span className="flex min-w-0 shrink items-center gap-1">
                                {commit.head && (
                                  <Badge variant="default" className="shrink-0 px-1 py-0 text-[10px]">
                                    HEAD
                                  </Badge>
                                )}
                                {commit.refs.map((ref) => {
                                  const badge = refBadge(ref)
                                  return (
                                    <Badge
                                      key={ref}
                                      variant={badge.variant}
                                      style={{ color: paint, borderColor: paint }}
                                      title={badge.label}
                                      className="max-w-28 min-w-9 shrink px-1 py-0 text-[10px]"
                                    >
                                      <span className="min-w-0 truncate">{badge.label}</span>
                                    </Badge>
                                  )
                                })}
                              </span>
                            )}
                          </span>
                          <span className="w-2 shrink-0" aria-hidden />
                          <span
                            className="shrink-0 truncate px-2 text-xs text-muted"
                            style={{ width: dateWidth }}
                            title={formatTime(commit.time)}
                          >
                            {formatRelative(commit.time)}
                          </span>
                          <span className="w-2 shrink-0" aria-hidden />
                          <span
                            className="shrink-0 truncate px-2 text-xs"
                            style={{ width: authorWidth }}
                            title={commit.author}
                          >
                            {commit.author}
                          </span>
                          <span className="w-2 shrink-0" aria-hidden />
                          <span
                            className="shrink-0 truncate px-2 font-mono text-xs"
                            style={{ width: commitWidth, color: paint }}
                            title={commit.id}
                          >
                            {commit.id.slice(0, 8)}
                          </span>
                        </button>
                      </ContextMenuTrigger>
                      <ContextMenuContent>
                        <ContextMenuItem onSelect={() => copy(commit.id)}>Copy hash</ContextMenuItem>
                        <ContextMenuItem onSelect={() => copy(commit.subject)}>Copy subject</ContextMenuItem>
                      </ContextMenuContent>
                    </ContextMenu>
                  </li>
                )
              })}
              {hasNextPage && (
                <li ref={sentinelRef} className="p-2">
                  <Button
                    variant="outline"
                    size="sm"
                    className="w-full"
                    disabled={isFetchingNextPage}
                    onClick={onLoadMore}
                  >
                    {isFetchingNextPage ? 'Loading…' : 'Load more commits'}
                  </Button>
                </li>
              )}
            </ul>
            <svg
              aria-hidden="true"
              className="pointer-events-none absolute left-0 top-0 overflow-hidden"
              width={graphWidth}
              height={svgHeight}
            >
              {uncommitted && (
                <g>
                  {headRow && (
                    <line
                      x1={laneX(headRow.lane)}
                      y1={ROW_H / 2}
                      x2={laneX(headRow.lane)}
                      y2={rowCy(headIndex)}
                      stroke={GRAPH_PALETTE[headRow.color]}
                      strokeWidth={2}
                      strokeLinecap="round"
                      strokeDasharray="4 3"
                    />
                  )}
                  <circle
                    cx={laneX(headRow?.lane ?? 0)}
                    cy={ROW_H / 2}
                    r={4}
                    fill={headRow ? GRAPH_PALETTE[headRow.color] : 'var(--muted)'}
                  />
                </g>
              )}
              {rows.map((row, index) => {
                const top = rowTop(index)
                const cy = rowCy(index)
                const bottom = rowBottom(index)
                const paint = GRAPH_PALETTE[row.color]
                const cx = laneX(row.lane)
                return (
                  <g key={index}>
                    {row.through.map((line, lineIndex) => (
                      <line
                        key={`through-${lineIndex}`}
                        x1={laneX(line.lane)}
                        y1={top}
                        x2={laneX(line.lane)}
                        y2={bottom}
                        stroke={GRAPH_PALETTE[line.color]}
                        strokeWidth={2}
                        strokeLinecap="round"
                      />
                    ))}
                    {row.incoming && (
                      <line
                        x1={cx}
                        y1={top}
                        x2={cx}
                        y2={cy}
                        stroke={paint}
                        strokeWidth={2}
                        strokeLinecap="round"
                      />
                    )}
                    {row.edges.map((edge, edgeIndex) => {
                      const x1 = laneX(edge.fromLane)
                      const x2 = laneX(edge.toLane)
                      const edgePaint = GRAPH_PALETTE[edge.color]
                      if (edge.fromLane === edge.toLane) {
                        return (
                          <line
                            key={`edge-${edgeIndex}`}
                            x1={x1}
                            y1={cy}
                            x2={x2}
                            y2={bottom}
                            stroke={edgePaint}
                            strokeWidth={2}
                            strokeLinecap="round"
                          />
                        )
                      }
                      const midY = (cy + bottom) / 2
                      return (
                        <path
                          key={`edge-${edgeIndex}`}
                          d={`M ${x1} ${cy} L ${x1} ${midY} C ${x1} ${bottom} ${x2} ${midY} ${x2} ${bottom}`}
                          fill="none"
                          stroke={edgePaint}
                          strokeWidth={2}
                          strokeLinecap="round"
                        />
                      )
                    })}
                    {row.head ? (
                      <g>
                        <circle cx={cx} cy={cy} r={5} fill="var(--bg)" stroke={paint} strokeWidth={2} />
                        {row.merge && <circle cx={cx} cy={cy} r={1.5} fill={paint} />}
                      </g>
                    ) : row.merge ? (
                      <g>
                        <circle cx={cx} cy={cy} r={4} fill={paint} />
                        <circle cx={cx} cy={cy} r={1.5} fill="var(--bg)" />
                      </g>
                    ) : (
                      <circle cx={cx} cy={cy} r={4} fill={paint} />
                    )}
                  </g>
                )
              })}
            </svg>
          </div>
        </div>
      </div>
    </div>
  )
}
