import { useEffect, useState } from 'react'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'
import { ErrorPane, LoadingPane, StaleNotice } from './panes'
import { useGitSearch } from './queries'

export function EditorSearch({
  projectId,
  checkoutPath,
  active,
  query,
  onSelect,
}: {
  projectId: string
  checkoutPath: string
  active: boolean
  query: string
  onSelect: (path: string, line: number) => void
}) {
  const [settledQuery, setSettledQuery] = useState('')
  const normalizedQuery = query.trim()
  const search = useGitSearch(projectId, checkoutPath, settledQuery, active && settledQuery.length >= 2)

  useEffect(() => {
    const timeout = window.setTimeout(() => setSettledQuery(normalizedQuery), 250)
    return () => window.clearTimeout(timeout)
  }, [normalizedQuery])

  if (normalizedQuery.length < 2) {
    return <p className="m-0 p-3 text-xs text-muted">Type at least 2 characters to search file contents.</p>
  }
  if (normalizedQuery !== settledQuery) {
    return <div role="status" className="p-3 text-xs text-muted">Waiting for typing to stop…</div>
  }
  if (search.isPending) return <LoadingPane label="Searching file contents…" />
  if (search.isError && !search.data) {
    return <ErrorPane error={search.error} onRetry={() => void search.refetch()} />
  }

  const matches = search.data?.matches ?? []
  // The previous query's rows stay up while the new one loads; dim them so they are not mistaken for its results.
  const stale = search.isPlaceholderData
  return (
    <div aria-busy={stale} className={cn('flex min-h-0 flex-1 flex-col transition-opacity', stale && 'opacity-50')}>
      {search.isError && <StaleNotice error={search.error} />}
      <ScrollArea className="min-h-0 flex-1">
        <ul className="m-0 list-none p-1.5">
          {matches.map((match, index) => (
            <li key={`${match.path}:${match.line}:${index}`}>
              <button
                type="button"
                data-nav-item
                onClick={() => onSelect(match.path, match.line)}
                className="flex w-full items-start gap-2 rounded px-1.5 py-1 text-left outline-none hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-ring"
                title={`${match.path}:${match.line}`}
              >
                <span className="shrink-0 font-mono text-xs text-accent">{match.path}:{match.line}</span>
                <span className="min-w-0 truncate font-mono text-xs text-muted">{match.text}</span>
              </button>
            </li>
          ))}
          {matches.length === 0 && <li className="px-1.5 py-2 text-xs text-muted">No matches.</li>}
        </ul>
      </ScrollArea>
      {search.data?.truncated && (
        <p role="status" className="m-0 shrink-0 border-t border-border px-2 py-1.5 text-xs text-muted">
          Showing first 500 matches.
        </p>
      )}
    </div>
  )
}
