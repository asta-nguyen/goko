import { useQueryClient } from '@tanstack/react-query'
import { ChevronRightIcon, FileIcon, FileWarningIcon, FolderIcon } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { cn } from '@/lib/utils'
import { queryKeys } from '../../queryClient'
import { gitViewService, type GitDiffRequest } from '../../services/git-view'
import { toBridgeError } from '../../services/errors'
import type { GitViewDiffLayer } from '../../services/types'
import { useUi } from '../../stores/ui'
import { CodeView, EmptyPane, ErrorPane, FilterInput, LoadingPane, matchesQuery, RefreshButton, SplitPane, StaleNotice } from './panes'
import { MarkdownView, isMarkdownPath } from './MarkdownView'
import { useEditorVim, useGitChanges, useGitFile, useGitFileList, useGitTree } from './queries'
import { CodeEditor } from './CodeEditor'
import { configCandidates, parseTsconfigPaths, pickDefinition, resolveImport, type GoToTarget, type PathAlias } from './go-to-definition'
import { EditorSearch } from './EditorSearch'

const sizeLabel = (size: number) => (size >= 1024 * 1024 ? `${(size / 1024 / 1024).toFixed(1)} MiB` : `${Math.max(1, Math.round(size / 1024))} KiB`)
const EDITOR_FILE_CAP = 1024 * 1024
const textForSave = (draft: { originalText: string; text: string }) => {
  const originalNewlines = draft.originalText.match(/\r\n|\r|\n/g) ?? []
  const editedNewlines = draft.text.match(/\r\n|\r|\n/g) ?? []
  if (originalNewlines.length === editedNewlines.length) {
    let index = 0
    return draft.text.replace(/\r\n|\r|\n/g, () => originalNewlines[index++] ?? '\n')
  }
  const newline = originalNewlines[0] ?? '\n'
  return draft.text.replace(/\r\n|\r|\n/g, newline)
}

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
                  data-nav-item
                  aria-expanded={isOpen}
                  onClick={() => onToggle(entry.path)}
                  style={indent}
                  className={cn(
                    'flex w-full items-center gap-1 rounded px-1 py-1 text-left outline-none hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-ring',
                    entry.ignored && 'opacity-50',
                  )}
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
                data-nav-item
                aria-pressed={selected === entry.path}
                onClick={() => onSelect(entry.path)}
                style={{ paddingLeft: depth * 12 + 18 }}
                className={cn(
                  'flex w-full items-center gap-1.5 rounded px-1 py-1 text-left outline-none hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-ring',
                  selected === entry.path && 'bg-secondary/70',
                  entry.ignored && 'opacity-50',
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
  openRequest,
  onOpenHandled,
}: {
  projectId: string
  checkoutPath: string
  active: boolean
  onShowChanges: (request: GitDiffRequest) => void
  openRequest?: { path: string; line?: number; nonce: number } | null
  onOpenHandled?: () => void
}) {
  const queryClient = useQueryClient()
  const [expanded, setExpanded] = useState<Record<string, boolean>>({})
  const [path, setPath] = useState<string | null>(null)
  const [filter, setFilter] = useState('')
  const [searchMode, setSearchMode] = useState<'name' | 'content'>('name')
  const [reveal, setReveal] = useState<{ line: number; nonce: number } | null>(null)
  const [mdView, setMdView] = useState<'preview' | 'raw'>('raw')
  const [saving, setSaving] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const lookup = useRef({ id: 0, scope: '' })
  lookup.current.scope = `${projectId}\0${checkoutPath}`
  const [vim] = useEditorVim()
  const editorDraft = useUi((state) => state.editorDraft)
  const editorSaveError = useUi((state) => state.editorSaveError)
  const setEditorDraft = useUi((state) => state.setEditorDraft)
  const setEditorSaveDraft = useUi((state) => state.setEditorSaveDraft)
  const setEditorReloadDraft = useUi((state) => state.setEditorReloadDraft)
  const requestEditorNavigation = useUi((state) => state.requestEditorNavigation)
  const file = useGitFile(projectId, checkoutPath, path, active)
  // No polling here — one fresh status read powers the "show changes" link.
  const changes = useGitChanges(projectId, checkoutPath, active, false)
  const fileList = useGitFileList(projectId, checkoutPath, active && searchMode === 'name' && filter.trim().length > 0)
  const filtered = useMemo(
    () =>
      searchMode === 'name' && filter.trim()
        ? (fileList.data ?? []).filter((item) => matchesQuery(item, filter)).slice(0, 200)
        : null,
    [fileList.data, filter, searchMode],
  )

  const saveDraft = useCallback(async () => {
    const draft = useUi.getState().editorDraft
    if (!draft) return true
    const text = textForSave(draft)
    if (text.includes('\0') || new TextEncoder().encode(text).byteLength > EDITOR_FILE_CAP) {
      useUi.getState().setEditorSaveError(
        text.includes('\0') ? 'Remove binary NUL characters before saving.' : 'Edited text exceeds the 1 MiB limit.',
      )
      return false
    }
    setSaving(true)
    try {
      const saved = await gitViewService.saveFile({
        projectId: draft.projectId,
        checkoutPath: draft.checkoutPath,
        path: draft.path,
        expectedText: draft.originalText,
        text,
      })
      queryClient.setQueryData(queryKeys.gitFile(draft.projectId, draft.checkoutPath, draft.path), saved)
      void queryClient.invalidateQueries({ queryKey: queryKeys.gitChanges(draft.projectId, draft.checkoutPath) })
      void queryClient.invalidateQueries({ queryKey: ['git-diff', draft.projectId, draft.checkoutPath] })
      useUi.getState().setEditorDraft(null)
      useUi.getState().setEditorSaveError(null)
      return true
    } catch (error) {
      useUi.getState().setEditorSaveError(toBridgeError(error).message)
      return false
    } finally {
      setSaving(false)
    }
  }, [queryClient])

  const reloadDraft = useCallback(async () => {
    const draft = useUi.getState().editorDraft
    if (!draft) return true
    try {
      const current = await gitViewService.file(draft.projectId, draft.checkoutPath, draft.path)
      queryClient.setQueryData(queryKeys.gitFile(draft.projectId, draft.checkoutPath, draft.path), current)
      useUi.getState().setEditorDraft(null)
      useUi.getState().setEditorSaveError(null)
      return true
    } catch (error) {
      useUi.getState().setEditorSaveError(toBridgeError(error).message)
      return false
    }
  }, [queryClient])

  useEffect(() => {
    setEditorSaveDraft(saveDraft)
    setEditorReloadDraft(reloadDraft)
    return () => {
      setEditorSaveDraft(null)
      setEditorReloadDraft(null)
    }
  }, [reloadDraft, saveDraft, setEditorReloadDraft, setEditorSaveDraft])

  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: ['git-tree', projectId, checkoutPath] })
    void queryClient.invalidateQueries({ queryKey: queryKeys.gitFileList(projectId, checkoutPath) })
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
  const draft = editorDraft &&
    editorDraft.projectId === projectId &&
    editorDraft.checkoutPath === checkoutPath &&
    editorDraft.path === path
    ? editorDraft
    : null
  const text = draft?.text ?? (file.data?.kind === 'text' ? file.data.text : null)
  const editable = file.data?.kind === 'text' && !file.data.truncated && text !== null
  const draftTooLarge = draft !== null && new TextEncoder().encode(textForSave(draft)).byteLength > EDITOR_FILE_CAP
  const draftHasNul = draft?.text.includes('\0') === true
  const markdown = text !== null && isMarkdownPath(file.data?.path)

  const selectPath = (nextPath: string, line?: number) => {
    if (nextPath === path) {
      if (line !== undefined) {
        setMdView('raw')
        setReveal((current) => ({ line, nonce: (current?.nonce ?? 0) + 1 }))
      }
      return
    }
    requestEditorNavigation(() => {
      setMdView('raw')
      setPath(nextPath)
      setReveal(line === undefined ? null : (current) => ({ line, nonce: (current?.nonce ?? 0) + 1 }))
    })
  }

  const handledOpenNonce = useRef<number | null>(null)
  useEffect(() => {
    if (!openRequest || openRequest.nonce === handledOpenNonce.current) return
    handledOpenNonce.current = openRequest.nonce
    selectPath(openRequest.path, openRequest.line)
    onOpenHandled?.()
  }, [openRequest, onOpenHandled])

  useEffect(() => {
    if (!notice) return
    const timer = window.setTimeout(() => setNotice(null), 4000)
    return () => window.clearTimeout(timer)
  }, [notice])

  /** `paths` aliases from the nearest config that defines any; unreadable configs are skipped. */
  const loadAliases = async (files: ReadonlySet<string>, fromPath: string): Promise<PathAlias[]> => {
    for (const config of configCandidates(fromPath).filter((candidate) => files.has(candidate))) {
      const file = await queryClient.fetchQuery({
        queryKey: queryKeys.gitFile(projectId, checkoutPath, config),
        queryFn: () => gitViewService.file(projectId, checkoutPath, config),
        staleTime: 30_000,
      })
      const aliases = file.kind === 'text' && file.text !== null ? parseTsconfigPaths(file.text, config) : []
      if (aliases.length) return aliases
    }
    return []
  }

  /** Cmd/Ctrl+click: open the imported file, or the one declaration of the name, else search for the name. */
  const goTo = async (target: GoToTarget) => {
    if (path === null) return
    const { scope } = lookup.current
    const id = ++lookup.current.id
    const stale = () => id !== lookup.current.id || scope !== lookup.current.scope
    setNotice(null)
    try {
      if (target.kind === 'import') {
        const files = new Set(
          await queryClient.fetchQuery({
            queryKey: queryKeys.gitFileList(projectId, checkoutPath),
            queryFn: () => gitViewService.fileList(projectId, checkoutPath),
            staleTime: 30_000,
          }),
        )
        const resolved = resolveImport(files, path, target.specifier, await loadAliases(files, path))
        if (stale()) return
        if (resolved) selectPath(resolved)
        else setNotice(`No file for '${target.specifier}' in this checkout`)
        return
      }
      const result = await queryClient.fetchQuery({
        queryKey: queryKeys.gitSearch(projectId, checkoutPath, target.name),
        queryFn: () => gitViewService.search(projectId, checkoutPath, target.name),
        staleTime: 30_000,
      })
      if (stale()) return
      const hit = pickDefinition(result.matches, target.name, path)
      if (hit) {
        selectPath(hit.path, hit.line)
      } else {
        setSearchMode('content')
        setFilter(target.name)
      }
    } catch (error) {
      if (!stale()) setNotice(toBridgeError(error).message)
    }
  }

  const updateText = (nextText: string) => {
    if (!editable || !file.data || file.data.text === null || path === null) return
    setEditorDraft({
      projectId,
      checkoutPath,
      path,
      originalText: draft?.originalText ?? file.data.text,
      text: nextText,
    })
  }

  return (
    <SplitPane
      aside={
        <>
          <header className="flex shrink-0 items-center justify-between border-b border-border px-2 py-1.5">
            <span className="text-[11px] font-semibold uppercase tracking-wider text-muted">Files</span>
            <RefreshButton label="Refresh file tree" isFetching={file.isFetching} onClick={refresh} />
          </header>
          <div className="flex shrink-0 items-center gap-2 border-b border-border px-2 py-1">
            <ToggleGroup
              type="single"
              size="sm"
              value={searchMode}
              aria-label="File search mode"
              onValueChange={(value) => {
                if (value === 'name' || value === 'content') setSearchMode(value)
              }}
            >
              <ToggleGroupItem value="name">Name</ToggleGroupItem>
              <ToggleGroupItem value="content">Content</ToggleGroupItem>
            </ToggleGroup>
            <FilterInput
              label={searchMode === 'name' ? 'Filter files' : 'Search file contents'}
              value={filter}
              onChange={setFilter}
            />
          </div>
          {searchMode === 'content' ? (
            <EditorSearch
              projectId={projectId}
              checkoutPath={checkoutPath}
              active={active}
              query={filter}
              onSelect={selectPath}
            />
          ) : (
            <ScrollArea className="min-h-0 flex-1">
              <div className="p-1.5">
                {filtered === null ? (
                  <TreeRows
                    projectId={projectId}
                    checkoutPath={checkoutPath}
                    directory=""
                    depth={0}
                    expanded={expanded}
                    onToggle={(directory) => setExpanded((current) => ({ ...current, [directory]: !current[directory] }))}
                    selected={path}
                    onSelect={selectPath}
                    active={active}
                  />
                ) : (
                  <ul className="m-0 list-none p-0">
                    {filtered.map((item) => (
                      <li key={item}>
                        <button
                          type="button"
                          data-nav-item
                          aria-pressed={path === item}
                          onClick={() => selectPath(item)}
                          className={cn(
                            'flex w-full items-center gap-1.5 rounded px-1 py-1 text-left outline-none hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-ring',
                            path === item && 'bg-secondary/70',
                          )}
                        >
                          <FileIcon className="size-3.5 shrink-0 text-muted" aria-hidden />
                          <span className="truncate font-mono text-xs" title={item}>{item}</span>
                        </button>
                      </li>
                    ))}
                    {filtered.length === 0 && (
                      <li className="px-1 py-1 text-xs text-muted">
                        {fileList.isPending ? 'Loading…' : fileList.isError ? 'Could not list files.' : 'No files match.'}
                      </li>
                    )}
                  </ul>
                )}
              </div>
            </ScrollArea>
          )}
        </>
      }
    >
      {path === null ? (
        <EmptyPane icon={<FileIcon className="size-4" />} title="Select a file to edit it" />
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
            {draft && <Badge variant="warning">Unsaved</Badge>}
            {draftTooLarge && <Badge variant="warning">over 1 MiB</Badge>}
            {draftHasNul && <Badge variant="warning">binary NUL</Badge>}
            {markdown && (
              <ToggleGroup
                type="single"
                size="sm"
                value={mdView}
                aria-label="Markdown view"
                onValueChange={(value) => {
                  if (value === 'preview' || value === 'raw') setMdView(value)
                }}
              >
                <ToggleGroupItem value="preview">Preview</ToggleGroupItem>
                <ToggleGroupItem value="raw">Raw</ToggleGroupItem>
              </ToggleGroup>
            )}
            {editable && (
              <Button variant="outline" size="sm" disabled={!draft || saving || draftTooLarge || draftHasNul} onClick={() => void saveDraft()}>
                {saving ? 'Saving…' : 'Save'}
              </Button>
            )}
            {changedLayer && (
              <Button variant="outline" size="sm" onClick={() => onShowChanges({ layer: changedLayer, path })}>
                Show changes
              </Button>
            )}
          </header>
          {notice && (
            <div role="status" className="shrink-0 border-b border-border bg-panel px-3 py-1 text-xs text-muted">{notice}</div>
          )}
          {editorSaveError && (
            <div role="alert" className="flex shrink-0 items-center gap-2 border-b border-border bg-danger/10 px-3 py-1.5 text-xs text-danger">
              <span className="min-w-0 flex-1">{editorSaveError}</span>
              {draft && <Button variant="ghost" size="xs" disabled={file.isFetching} onClick={() => void reloadDraft()}>Reload from disk</Button>}
            </div>
          )}
          {text !== null ? (
            editable && markdown && mdView === 'preview' ? (
              <MarkdownView text={text} />
            ) : editable ? (
              <CodeEditor
                key={`${checkoutPath}:${path}`}
                value={text}
                path={file.data.path}
                ariaLabel={`Edit ${file.data.path}`}
                readOnly={saving}
                vim={vim}
                reveal={reveal}
                onRevealed={() => setReveal(null)}
                onChange={updateText}
                onSave={() => void saveDraft()}
                onGoTo={(target) => void goTo(target)}
              />
            ) : markdown && mdView === 'preview' ? (
              <MarkdownView text={text} />
            ) : (
              <CodeView text={text} path={file.data.path} />
            )
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
