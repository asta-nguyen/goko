import { RefreshCwIcon } from 'lucide-react'
import { useMemo } from 'react'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ResizableSplit } from '@/components/ui/resizable-split'
import { Spinner } from '@/components/ui/spinner'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { cn } from '@/lib/utils'
import { isBridgeError } from '../../services/errors'
import type { GitViewStatusEntry } from '../../services/types'
import { PANEL_LIMITS, useUi } from '../../stores/ui'
import { toSplitRows, toUnifiedLines, type DiffRow, type UnifiedLine } from './diff-view'
import { langForPath, useHighlightedDiffLines, useHighlightedLines } from './highlight'

export const errorMessage = (error: unknown) =>
  isBridgeError(error) ? error.message : error instanceof Error ? error.message : String(error)

export const formatTime = (epochMs: number) =>
  new Date(epochMs).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })

export const statusLabel = (status: GitViewStatusEntry['status']) =>
  status === 'typechange' ? 'type changed' : status

export const entryName = (entry: GitViewStatusEntry) =>
  entry.orig_path ? `${entry.orig_path} → ${entry.path}` : entry.path

/** Section body: a resizable navigation/list column and the detail pane. */
export function SplitPane({ aside, children }: { aside: React.ReactNode; children: React.ReactNode }) {
  const width = useUi((state) => state.panelWidths.editorList)
  const setPanelWidth = useUi((state) => state.setPanelWidth)
  return (
    <ResizableSplit
      label="Resize list and detail panes"
      width={width}
      min={PANEL_LIMITS.editorList.minWidth}
      max={PANEL_LIMITS.editorList.maxWidth}
      siblingMin={PANEL_LIMITS.editorList.siblingMin}
      onResize={(next) => setPanelWidth('editorList', next)}
      aside={aside}
      asideClassName="bg-panel/40"
    >
      {children}
    </ResizableSplit>
  )
}

/** Case-insensitive match where every whitespace-separated term must appear. */
export function matchesQuery(text: string, query: string): boolean {
  const haystack = text.toLowerCase()
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((term) => haystack.includes(term))
}

export function FilterInput({
  label,
  value,
  onChange,
}: {
  label: string
  value: string
  onChange: (value: string) => void
}) {
  return (
    <Input
      aria-label={label}
      placeholder="Filter…"
      data-editor-filter
      value={value}
      onChange={(event) => onChange(event.target.value)}
      spellCheck={false}
      autoCapitalize="off"
      autoCorrect="off"
      className="h-6 px-2 text-xs"
    />
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

/** Renders highlighted HTML when available and the plain text otherwise. */
function Highlighted({ text, html }: { text: string; html: string | null | undefined }) {
  if (html === null || html === undefined) return <span className="whitespace-pre">{text}</span>
  return <span className="whitespace-pre" dangerouslySetInnerHTML={{ __html: html }} />
}

const unifiedLineClass = (kind: UnifiedLine['kind']) =>
  kind === 'del' ? 'bg-danger/10 text-danger' : kind === 'add' ? 'bg-success/10 text-success' : ''

/** Switches every diff pane between the unified and the two-column view. */
export function DiffViewToggle() {
  const diffView = useUi((state) => state.diffView)
  const setDiffView = useUi((state) => state.setDiffView)
  return (
    <ToggleGroup
      type="single"
      size="sm"
      value={diffView}
      aria-label="Diff view"
      onValueChange={(value) => {
        if (value === 'unified' || value === 'split') setDiffView(value)
      }}
    >
      <ToggleGroupItem value="unified">Unified</ToggleGroupItem>
      <ToggleGroupItem value="split">Split</ToggleGroupItem>
    </ToggleGroup>
  )
}

const splitCellClass = (row: DiffRow, side: 'left' | 'right') => {
  if (row.kind === 'context') return ''
  if (side === 'left') return row.kind === 'add' ? 'bg-bg/40' : 'bg-danger/10 text-danger'
  return row.kind === 'del' ? 'bg-bg/40' : 'bg-success/10 text-success'
}

function SplitDiff({ rows, html }: { rows: DiffRow[]; html: (string | null)[] | null }) {
  let cursor = 0
  const next = () => {
    const value = html === null ? null : html[cursor]
    cursor++
    return value
  }
  return (
    <div className="min-w-max p-3 font-mono text-xs leading-5">
      {rows.map((row, index) => {
        if (row.kind === 'meta' || row.kind === 'hunk') {
          next()
          return (
            <span key={index} className={cn('block whitespace-pre px-3', row.kind === 'hunk' ? 'text-accent' : 'text-muted')}>
              {row.text}
            </span>
          )
        }
        const leftHtml = next()
        const rightHtml = next()
        return (
          <div key={index} className="grid grid-cols-2">
            <span className={cn('flex whitespace-pre', splitCellClass(row, 'left'))}>
              <span aria-hidden className="w-10 shrink-0 select-none pr-2 text-right text-muted">
                {row.kind === 'add' ? '' : row.left.no}
              </span>
              <span className="min-w-0 pr-2">
                <Highlighted text={row.kind === 'add' ? '' : row.left.text} html={leftHtml} />
              </span>
            </span>
            <span className={cn('flex whitespace-pre border-l border-border', splitCellClass(row, 'right'))}>
              <span aria-hidden className="w-10 shrink-0 select-none pr-2 text-right text-muted">
                {row.kind === 'del' ? '' : row.right.no}
              </span>
              <span className="min-w-0 pr-2">
                <Highlighted text={row.kind === 'del' ? '' : row.right.text} html={rightHtml} />
              </span>
            </span>
          </div>
        )
      })}
    </div>
  )
}

/** A diff in the unified or two-column mode chosen in the Editor header. */
export function DiffPatch({ patch, truncated }: { patch: string; truncated: boolean }) {
  const diffView = useUi((state) => state.diffView)
  const rows = useMemo(() => toSplitRows(patch), [patch])
  const unified = useMemo(() => toUnifiedLines(rows), [rows])
  const unifiedTexts = useMemo(
    () => unified.map((line) => (line.kind === 'meta' || line.kind === 'hunk' ? '' : line.text)),
    [unified],
  )
  const unifiedLangs = useMemo(
    () => unified.map((line) => (line.kind === 'meta' || line.kind === 'hunk' ? null : langForPath(line.file))),
    [unified],
  )
  const splitTexts = useMemo(
    () =>
      rows.flatMap((row) =>
        row.kind === 'meta' || row.kind === 'hunk'
          ? [row.text]
          : [row.kind === 'add' ? '' : row.left.text, row.kind === 'del' ? '' : row.right.text],
      ),
    [rows],
  )
  const splitLangs = useMemo(
    () =>
      rows.flatMap((row) =>
        row.kind === 'meta' || row.kind === 'hunk' ? [null] : [langForPath(row.file), langForPath(row.file)],
      ),
    [rows],
  )
  const split = diffView === 'split'
  const html = useHighlightedDiffLines(split ? splitTexts : unifiedTexts, split ? splitLangs : unifiedLangs)
  return (
    <div className="min-h-0 flex-1 overflow-auto">
      {split ? (
        <SplitDiff rows={rows} html={html} />
      ) : (
        <div className="min-w-max p-3 font-mono text-xs leading-5">
          {unified.map((line, index) =>
            line.kind === 'meta' || line.kind === 'hunk' ? (
              <span key={index} className={cn('block whitespace-pre px-3', line.kind === 'hunk' ? 'text-accent' : 'text-muted')}>
                {line.text}
              </span>
            ) : (
              <div key={index} className={cn('flex whitespace-pre', unifiedLineClass(line.kind))}>
                <span aria-hidden className="w-10 shrink-0 select-none pr-2 text-right text-muted">
                  {line.no}
                </span>
                <span className="shrink-0 select-none pr-2">{line.prefix}</span>
                <Highlighted text={line.text} html={html?.[index]} />
              </div>
            ),
          )}
        </div>
      )}
      {truncated && (
        <p role="status" className="sticky bottom-0 m-0 border-t border-border bg-warning/10 px-3 py-1.5 text-xs text-warning">
          This diff is larger than 1 MiB — only the first part is shown.
        </p>
      )}
    </div>
  )
}

/** Bounded, read-only, line-numbered code view with syntax highlighting. */
export function CodeView({ text, path }: { text: string; path?: string }) {
  const lines = useMemo(() => text.split('\n'), [text])
  const html = useHighlightedLines(lines, langForPath(path), 'block')
  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <code className="block min-w-max p-3 font-mono text-xs leading-5">
        {lines.map((line, index) => (
          <div key={index} className="flex">
            <span aria-hidden className="w-12 shrink-0 select-none pr-3 text-right text-muted">
              {index + 1}
            </span>
            {line === '' ? (
              <span className="whitespace-pre"> </span>
            ) : (
              <Highlighted text={line} html={html?.[index]} />
            )}
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
