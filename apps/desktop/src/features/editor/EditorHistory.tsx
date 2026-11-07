import { useQueryClient } from '@tanstack/react-query'
import { GitCommitHorizontalIcon, HistoryIcon } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { ResizableHandle } from '@/components/ui/resizable-handle'
import { queryKeys } from '../../queryClient'
import { CommitDetail, UncommittedDetail } from './CommitDetail'
import { HistoryTable, type HistoryTableHandle } from './HistoryTable'
import { LAYOUT_LIMITS, useHistoryLayout } from './history-layout'
import { EmptyPane, ErrorPane, FilterInput, LoadingPane, matchesQuery, RefreshButton, StaleNotice } from './panes'
import { useGitBranches, useGitChanges, useGitDiff, useGitHistory } from './queries'

const TABLE_MIN_HEIGHT = 120
const DETAIL_HANDLE_HEIGHT = 8
// Until the container is measured the detail may grow to any sane height.
const UNMEASURED_DETAIL_MAX = 100_000

export function EditorHistory({
  projectId,
  checkoutPath,
  active,
  onOpenChanges,
}: {
  projectId: string
  checkoutPath: string
  active: boolean
  onOpenChanges: () => void
}) {
  const queryClient = useQueryClient()
  const { layout, setColumn, setDetailHeight, reset } = useHistoryLayout()
  const [branch, setBranch] = useState<string | null>(null)
  const [selected, setSelected] = useState<string | null>(null)
  const [uncommittedChosen, setUncommittedChosen] = useState(false)
  const [filter, setFilter] = useState('')
  const tableRef = useRef<HistoryTableHandle>(null)
  const [container, setContainer] = useState<HTMLDivElement | null>(null)
  const [containerHeight, setContainerHeight] = useState<number | null>(null)

  const history = useGitHistory(projectId, checkoutPath, branch, active)
  const branches = useGitBranches(projectId, checkoutPath, active)
  const changes = useGitChanges(projectId, checkoutPath, active, false)
  const commits = useMemo(() => history.data?.pages.flatMap((page) => page.commits) ?? [], [history.data])

  useEffect(() => {
    if (!container || typeof ResizeObserver === 'undefined') return
    const update = () => {
      const height = container.getBoundingClientRect().height
      if (height) setContainerHeight(Math.round(height))
    }
    update()
    const observer = new ResizeObserver(update)
    observer.observe(container)
    return () => observer.disconnect()
  }, [container])

  const matches = useMemo(
    () =>
      filter.trim()
        ? new Set(
            commits
              .filter((commit) => matchesQuery(`${commit.id} ${commit.subject} ${commit.author}`, filter))
              .map((commit) => commit.id),
          )
        : null,
    [commits, filter],
  )

  const commit = commits.find((item) => item.id === selected) ?? null
  const loadedIds = useMemo(() => new Set(commits.map((item) => item.id)), [commits])
  const diff = useGitDiff(projectId, checkoutPath, selected ? { layer: 'commit', commit: selected } : null, {
    enabled: active,
  })

  const uncommittedEntries = useMemo(() => {
    const data = changes.data
    return data ? [...data.conflicts, ...data.staged, ...data.unstaged, ...data.untracked] : []
  }, [changes.data])
  const uncommittedVisible = uncommittedEntries.length > 0 && commits.some((item) => item.head)
  // A chosen row that disappears (changes committed elsewhere) stops counting as chosen.
  const uncommittedSelected = uncommittedChosen && uncommittedVisible
  const uncommitted = uncommittedVisible ? { count: uncommittedEntries.length, selected: uncommittedSelected } : null

  useEffect(() => {
    if (selected === null && !uncommittedSelected && commits.length > 0) {
      setSelected(commits.find((commit) => commit.head)?.id ?? commits[0].id)
    }
  }, [commits, selected, uncommittedSelected])

  const selectCommit = (id: string) => {
    setUncommittedChosen(false)
    setSelected(id)
  }
  const selectUncommitted = () => {
    setSelected(null)
    setUncommittedChosen(true)
  }

  const jumpToMatch = (direction: 1 | -1) => {
    if (matches === null || matches.size === 0 || commits.length === 0) return
    const current = commits.findIndex((item) => item.id === selected)
    const start = current === -1 ? (direction === 1 ? -1 : commits.length) : current
    for (let step = 1; step <= commits.length; step++) {
      const index = (((start + direction * step) % commits.length) + commits.length) % commits.length
      const id = commits[index].id
      if (!matches.has(id)) continue
      selectCommit(id)
      tableRef.current?.scrollTo(id)
      return
    }
  }

  const onFilterKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Enter') return
    jumpToMatch(event.shiftKey ? -1 : 1)
  }

  const headCommit = commits.find((item) => item.head) ?? null
  const jumpToHead = () => {
    if (headCommit === null) return
    selectCommit(headCommit.id)
    tableRef.current?.scrollTo(headCommit.id)
  }

  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: queryKeys.gitHistory(projectId, checkoutPath) })
    void queryClient.invalidateQueries({ queryKey: queryKeys.gitBranches(projectId, checkoutPath) })
    void queryClient.invalidateQueries({ queryKey: queryKeys.gitChanges(projectId, checkoutPath) })
    void queryClient.invalidateQueries({ queryKey: ['git-diff', projectId, checkoutPath] })
  }

  const locals = (branches.data ?? []).filter((item) => !item.remote)
  const remotes = (branches.data ?? []).filter((item) => item.remote)

  const toolbar = (
    <div className="flex shrink-0 items-center gap-1.5 border-b border-border px-2 py-1">
      <select
        aria-label="Branch"
        value={branch ?? ''}
        onChange={(event) => {
          setBranch(event.target.value === '' ? null : event.target.value)
          setSelected(null)
          setUncommittedChosen(false)
          setFilter('')
        }}
        className="h-6 w-32 shrink-0 truncate rounded-md border border-input bg-bg px-1.5 text-xs text-text outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
      >
        <option value="">All branches</option>
        <optgroup label="Local">
          {locals.map((item) => (
            <option key={item.ref_name} value={item.ref_name}>
              {item.name}
            </option>
          ))}
        </optgroup>
        <optgroup label="Remote">
          {remotes.map((item) => (
            <option key={item.ref_name} value={item.ref_name}>
              {item.name}
            </option>
          ))}
        </optgroup>
      </select>
      <div className="min-w-0 flex-1" onKeyDown={onFilterKeyDown}>
        <FilterInput label="Filter commits" value={filter} onChange={setFilter} />
      </div>
      <Button variant="outline" size="sm" className="shrink-0" disabled={headCommit === null} onClick={jumpToHead}>
        Jump to HEAD
      </Button>
      <RefreshButton label="Refresh history" isFetching={history.isFetching} onClick={refresh} />
    </div>
  )

  const detailMin = LAYOUT_LIMITS.detailHeight.min
  const detailMax =
    containerHeight === null
      ? UNMEASURED_DETAIL_MAX
      : Math.max(detailMin, containerHeight - TABLE_MIN_HEIGHT - DETAIL_HANDLE_HEIGHT)
  const detailHeight = Math.max(detailMin, Math.min(detailMax, layout.detailHeight))

  const detail = uncommittedSelected ? (
    <UncommittedDetail entries={uncommittedEntries} onOpenChanges={onOpenChanges} />
  ) : commit === null ? (
    <EmptyPane icon={<GitCommitHorizontalIcon className="size-4" />} title="Select a commit to see its patch" />
  ) : diff.isPending ? (
    <LoadingPane label="Loading commit…" />
  ) : diff.isError && !diff.data ? (
    <ErrorPane error={diff.error} onRetry={() => void diff.refetch()} />
  ) : diff.data ? (
    <CommitDetail
      commit={commit}
      diff={diff.data}
      stale={diff.isError ? diff.error : null}
      loadedIds={loadedIds}
      onSelectParent={selectCommit}
    />
  ) : null

  if (!history.isPending && !(history.isError && !history.data) && commits.length === 0 && branch === null) {
    return (
      <EmptyPane icon={<HistoryIcon className="size-4" />} title="No commits yet">
        <p className="m-0">This repository has no commits on its local, remote-tracking, or tag refs.</p>
        <RefreshButton label="Refresh history" isFetching={history.isFetching} onClick={refresh} />
      </EmptyPane>
    )
  }

  return (
    <div ref={setContainer} className="flex min-h-0 flex-1 flex-col">
      {toolbar}
      {history.isPending ? (
        <LoadingPane label="Loading history…" />
      ) : history.isError && !history.data ? (
        <ErrorPane error={history.error} onRetry={() => void history.refetch()} />
      ) : (
        <>
          {history.isError && <StaleNotice error={history.error} />}
          <div className="flex min-h-0 flex-1 flex-col" style={{ minHeight: TABLE_MIN_HEIGHT }}>
            <HistoryTable
              ref={tableRef}
              commits={commits}
              selected={selected}
              onSelect={selectCommit}
              matches={matches}
              hasNextPage={history.hasNextPage}
              isFetchingNextPage={history.isFetchingNextPage}
              onLoadMore={() => void history.fetchNextPage()}
              uncommitted={uncommitted}
              onSelectUncommitted={selectUncommitted}
              layout={layout}
              onColumn={setColumn}
              onResetColumn={reset}
            />
          </div>
          <ResizableHandle
            orientation="horizontal"
            label="Resize history table and detail"
            invert
            value={detailHeight}
            min={detailMin}
            max={detailMax}
            onChange={setDetailHeight}
            onReset={() => reset('detailHeight')}
          />
          <div className="flex shrink-0 flex-col overflow-hidden" style={{ height: detailHeight }}>
            {detail}
          </div>
        </>
      )}
    </div>
  )
}
