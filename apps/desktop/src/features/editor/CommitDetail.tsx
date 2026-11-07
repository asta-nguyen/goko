import { CopyIcon } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import type { GitViewCommit, GitViewDiff, GitViewStatusEntry } from '../../services/types'
import { ChangedFiles, DiffPatch, DiffViewToggle, entryName, formatTime, refBadge, StaleNotice, statusLabel } from './panes'

const COPY_FEEDBACK_MS = 1500

export function CommitDetail({
  commit,
  diff,
  stale,
  loadedIds,
  onSelectParent,
}: {
  commit: GitViewCommit
  diff: GitViewDiff
  stale: unknown | null
  loadedIds: Set<string>
  onSelectParent: (id: string) => void
}) {
  const [copyNote, setCopyNote] = useState<'Copied' | 'Copy failed' | null>(null)
  const copyTimer = useRef<number | null>(null)
  const [bodyExpanded, setBodyExpanded] = useState(false)

  useEffect(
    () => () => {
      if (copyTimer.current !== null) window.clearTimeout(copyTimer.current)
    },
    [],
  )
  useEffect(() => {
    setBodyExpanded(false)
    setCopyNote(null)
  }, [commit.id])

  const detail = diff.detail ?? null
  const authorEmail = detail?.author_email ?? ''
  const body = detail?.body ?? ''
  const committerLine =
    detail !== null && (detail.committer !== commit.author || detail.committer_email !== detail.author_email)
      ? `Committer: ${detail.committer}${detail.committer_email ? ` <${detail.committer_email}>` : ''}`
      : null

  const copyHash = () => {
    const finish = (ok: boolean) => {
      setCopyNote(ok ? 'Copied' : 'Copy failed')
      if (copyTimer.current !== null) window.clearTimeout(copyTimer.current)
      copyTimer.current = window.setTimeout(() => setCopyNote(null), COPY_FEEDBACK_MS)
    }
    const clipboard = navigator.clipboard
    if (!clipboard) {
      finish(false)
      return
    }
    clipboard.writeText(commit.id).then(() => finish(true), () => finish(false))
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-auto">
        <header className="border-b border-border px-3 py-1.5 text-xs">
          <div className="flex items-center gap-1.5">
            <span className="font-mono">{commit.id.slice(0, 8)}</span>
            <Button variant="ghost" size="icon-xs" aria-label="Copy hash" title="Copy hash" onClick={copyHash}>
              <CopyIcon aria-hidden />
            </Button>
            {copyNote !== null && <span className="text-muted">{copyNote}</span>}
            <span className="ml-auto">
              <DiffViewToggle />
            </span>
          </div>
          <p className="m-0 mt-0.5 font-medium">{commit.subject}</p>
          {(commit.head || commit.refs.length > 0) && (
            <div className="mt-1 flex flex-wrap items-center gap-1">
              {commit.head && (
                <Badge variant="default" className="px-1 py-0 text-[10px]">
                  HEAD
                </Badge>
              )}
              {commit.refs.map((ref) => {
                const badge = refBadge(ref)
                return (
                  <Badge key={ref} variant={badge.variant} title={badge.label} className="max-w-40 px-1 py-0 text-[10px]">
                    <span className="min-w-0 truncate">{badge.label}</span>
                  </Badge>
                )
              })}
            </div>
          )}
          <p className="m-0 mt-1 text-muted">
            {`${commit.author}${authorEmail ? ` <${authorEmail}>` : ''} · ${formatTime(commit.time)}`}
          </p>
          {committerLine !== null && <p className="m-0 mt-0.5 text-muted">{committerLine}</p>}
          {commit.parents.length > 0 && (
            <div className="mt-1 flex flex-wrap items-center gap-1">
              <span className="text-muted">Parents</span>
              {commit.parents.map((id) => (
                <Button
                  key={id}
                  variant="ghost"
                  size="xs"
                  className="font-mono"
                  aria-label={`Select parent ${id.slice(0, 8)}`}
                  disabled={!loadedIds.has(id)}
                  onClick={() => onSelectParent(id)}
                >
                  {id.slice(0, 8)}
                </Button>
              ))}
            </div>
          )}
          {body.trim() !== '' && (
            <div className="mt-1">
              <p className={cn('m-0 whitespace-pre-wrap text-muted', !bodyExpanded && 'line-clamp-6')}>{body}</p>
              {body.split('\n').length > 6 && (
                <Button
                  variant="ghost"
                  size="xs"
                  className="mt-0.5 px-1.5"
                  aria-expanded={bodyExpanded}
                  onClick={() => setBodyExpanded((value) => !value)}
                >
                  {bodyExpanded ? 'Show less' : 'Show more'}
                </Button>
              )}
            </div>
          )}
          {commit.parents.length > 1 && (
            <p className="m-0 mt-1 text-muted">Changes compared with the first parent</p>
          )}
        </header>
        {stale != null && <StaleNotice error={stale} />}
        <ChangedFiles files={diff.files} />
        <DiffPatch patch={diff.patch} truncated={diff.truncated} />
      </div>
    </div>
  )
}

export function UncommittedDetail({
  entries,
  onOpenChanges,
}: {
  entries: GitViewStatusEntry[]
  onOpenChanges: () => void
}) {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="flex shrink-0 items-center justify-between gap-2 border-b border-border px-3 py-1.5">
        <span className="text-xs font-medium">Uncommitted changes</span>
        <Button variant="outline" size="sm" onClick={onOpenChanges}>
          Open in Changes
        </Button>
      </header>
      <ul aria-label="Uncommitted entries" className="m-0 min-h-0 flex-1 list-none overflow-auto p-0">
        {entries.map((entry, index) => (
          <li key={`${entry.path}:${index}`} className="flex items-center gap-2 px-3 py-1 text-xs">
            <Badge variant="outline" className="shrink-0 font-sans">
              {statusLabel(entry.status)}
            </Badge>
            <span className="min-w-0 truncate" title={entryName(entry)}>
              {entryName(entry)}
            </span>
          </li>
        ))}
      </ul>
    </div>
  )
}
