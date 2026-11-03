import { RefreshCwIcon } from 'lucide-react'
import { useMemo } from 'react'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Spinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'
import { isBridgeError } from '../../services/errors'
import type { GitViewStatusEntry } from '../../services/types'

export const errorMessage = (error: unknown) =>
  isBridgeError(error) ? error.message : error instanceof Error ? error.message : String(error)

export const formatTime = (epochMs: number) =>
  new Date(epochMs).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })

export const statusLabel = (status: GitViewStatusEntry['status']) =>
  status === 'typechange' ? 'type changed' : status

export const entryName = (entry: GitViewStatusEntry) =>
  entry.orig_path ? `${entry.orig_path} → ${entry.path}` : entry.path

/** Section body: a fixed-width navigation/list column and the detail pane. */
export function SplitPane({ aside, children }: { aside: React.ReactNode; children: React.ReactNode }) {
  return (
    <div className="flex min-h-0 flex-1">
      <aside className="flex w-72 shrink-0 flex-col border-r border-border bg-panel/40">{aside}</aside>
      <div className="flex min-w-0 flex-1 flex-col">{children}</div>
    </div>
  )
}

export function RefreshButton({
  label,
  isFetching,
  onClick,
}: {
  label: string
  isFetching: boolean
  onClick: () => void
}) {
  return (
    <Button variant="ghost" size="icon-sm" aria-label={label} title={label} onClick={onClick}>
      <RefreshCwIcon className={isFetching ? 'animate-spin' : undefined} aria-hidden />
    </Button>
  )
}

export function LoadingPane({ label }: { label: string }) {
  return (
    <div role="status" className="flex flex-1 items-center justify-center gap-2 p-6 text-sm text-muted">
      <Spinner aria-hidden />
      {label}
    </div>
  )
}

/** A first-load failure: nothing to show yet, so the error gets a retry action. */
export function ErrorPane({ error, onRetry }: { error: unknown; onRetry: () => void }) {
  return (
    <div className="flex flex-1 items-center justify-center p-6">
      <Alert variant="destructive" className="max-w-md">
        <AlertDescription className="flex flex-col gap-2">
          <span>{errorMessage(error)}</span>
          {isBridgeError(error) && error.hint && <span className="text-xs">{error.hint}</span>}
          <Button variant="outline" size="sm" className="self-start" onClick={onRetry}>
            Retry
          </Button>
        </AlertDescription>
      </Alert>
    </div>
  )
}

/** A background refresh failure with a successful result still on screen. */
export function StaleNotice({ error }: { error: unknown }) {
  return (
    <p role="status" className="m-0 border-b border-border bg-warning/10 px-3 py-1.5 text-xs text-warning">
      Refresh failed — showing the last result. {errorMessage(error)}
    </p>
  )
}

export function EmptyPane({
  icon,
  title,
  children,
}: {
  icon: React.ReactNode
  title: string
  children?: React.ReactNode
}) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center">
      <span className="grid size-9 place-items-center rounded-lg bg-bg text-muted" aria-hidden>
        {icon}
      </span>
      <p className="m-0 text-sm font-medium">{title}</p>
      {children && <div className="text-xs text-muted">{children}</div>}
    </div>
  )
}

const diffLineClass = (line: string) => {
  if (line.startsWith('+') && !line.startsWith('+++')) return 'text-success'
  if (line.startsWith('-') && !line.startsWith('---')) return 'text-danger'
  if (line.startsWith('@@')) return 'text-accent'
  if (/^(diff --git|index |new file|deleted file|similarity|rename (from|to)|copy (from|to)|Binary files|old mode|new mode)/.test(line))
    return 'text-muted'
  return ''
}

/** A unified diff with +/- and header lines tinted. */
export function DiffPatch({ patch, truncated }: { patch: string; truncated: boolean }) {
  const chunks = useMemo(() => {
    const groups: { cls: string; text: string }[] = []
    for (const line of patch.split('\n')) {
      const cls = diffLineClass(line)
      const last = groups.at(-1)
      if (last && last.cls === cls) last.text += `\n${line}`
      else groups.push({ cls, text: line })
    }
    return groups
  }, [patch])
  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <pre className="min-w-max p-3 font-mono text-xs leading-5">
        {chunks.map((chunk, index) => (
          <span key={index} className={cn('block whitespace-pre', chunk.cls)}>
            {chunk.text}
          </span>
        ))}
      </pre>
      {truncated && (
        <p role="status" className="sticky bottom-0 m-0 border-t border-border bg-warning/10 px-3 py-1.5 text-xs text-warning">
          This diff is larger than 1 MiB — only the first part is shown.
        </p>
      )}
    </div>
  )
}

/** Bounded, read-only, line-numbered code view. */
export function CodeView({ text }: { text: string }) {
  const lines = useMemo(() => text.split('\n'), [text])
  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <code className="block min-w-max p-3 font-mono text-xs leading-5">
        {lines.map((line, index) => (
          <div key={index} className="flex">
            <span aria-hidden className="w-12 shrink-0 select-none pr-3 text-right text-muted">
              {index + 1}
            </span>
            <span className="whitespace-pre">{line === '' ? ' ' : line}</span>
          </div>
        ))}
      </code>
    </div>
  )
}

/** The changed-file list that commit and stash diffs carry. */
export function ChangedFiles({ files }: { files: GitViewStatusEntry[] }) {
  if (!files.length) return null
  return (
    <ul aria-label="Changed files" className="m-0 flex list-none flex-col gap-0.5 border-b border-border p-2">
      {files.map((file, index) => (
        <li key={`${file.path}:${index}`} className="flex items-center gap-2 px-1 font-mono text-xs">
          <Badge variant="outline" className="shrink-0 font-sans">
            {statusLabel(file.status)}
          </Badge>
          <span className="min-w-0 truncate" title={entryName(file)}>
            {file.orig_path && <span className="text-muted">{file.orig_path} → </span>}
            {file.path}
          </span>
        </li>
      ))}
    </ul>
  )
}
