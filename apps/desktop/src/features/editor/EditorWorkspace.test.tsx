import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient, queryKeys } from '../../queryClient'
import type { GitViewChanges, Project, WorktreeInfo } from '../../services/types'
import { useUi } from '../../stores/ui'
import { EditorWorkspace } from './EditorWorkspace'

const { service, settings, worktrees, gitWrite, acp } = vi.hoisted(() => ({
  service: {
    changes: vi.fn(),
    diff: vi.fn(),
    history: vi.fn(),
    tree: vi.fn(),
    fileList: vi.fn(),
    file: vi.fn(),
    search: vi.fn(),
    stashes: vi.fn(),
    stashDiff: vi.fn(),
  },
  settings: { get: vi.fn(), set: vi.fn() },
  worktrees: { list: vi.fn() },
  gitWrite: {
    stage: vi.fn(),
    unstage: vi.fn(),
    discard: vi.fn(),
    commit: vi.fn(),
    push: vi.fn(),
  },
  acp: {
    agents: vi.fn(),
    setAgents: vi.fn(),
    connect: vi.fn(),
    disconnect: vi.fn(),
    status: vi.fn(),
    generateCommitMessage: vi.fn(),
    cancel: vi.fn(),
  },
}))
vi.mock('../../services/git-view', () => ({ gitViewService: service }))
vi.mock('../../services/git-write', () => ({ gitWriteService: gitWrite }))
vi.mock('../../services/acp', () => ({ acpService: acp }))
vi.mock('../../services/settings', () => ({ settingsService: settings }))
vi.mock('../../services/worktrees', () => ({ worktreeService: worktrees }))
vi.mock('./CodeEditor', () => ({
  CodeEditor: ({
    value,
    ariaLabel,
    readOnly,
    vim,
    reveal,
    onRevealed,
    onChange,
    onGoTo,
  }: {
    value: string
    ariaLabel: string
    readOnly: boolean
    vim: boolean
    reveal: { line: number; nonce: number } | null
    onRevealed?: () => void
    onChange: (value: string) => void
    onGoTo?: (target: { kind: 'import'; specifier: string } | { kind: 'name'; name: string }) => void
  }) => (
    <>
      <textarea
        aria-label={ariaLabel}
        data-vim={String(vim)}
        data-reveal-line={reveal?.line}
        data-reveal-nonce={reveal?.nonce}
        value={value}
        readOnly={readOnly}
        onChange={(event) => onChange(event.target.value)}
      />
      {/* Stands in for the real editor finishing the cursor jump. */}
      <button type="button" data-testid="reveal-done" onClick={onRevealed} />
      {/* Stand in for Cmd/Ctrl+click on a token. */}
      <button type="button" data-testid="go-relative" onClick={() => onGoTo?.({ kind: 'import', specifier: './util' })} />
      <button type="button" data-testid="go-alias" onClick={() => onGoTo?.({ kind: 'import', specifier: '@/stores/ui' })} />
      <button type="button" data-testid="go-package" onClick={() => onGoTo?.({ kind: 'import', specifier: 'react' })} />
      <button type="button" data-testid="go-name" onClick={() => onGoTo?.({ kind: 'name', name: 'Foo' })} />
    </>
  ),
}))

const project = (over: Partial<Project> = {}): Project => ({
  id: 'p1',
  name: 'name-p1',
  root: '/work/p1',
  created_at: 1,
  missing: false,
  ...over,
})
const worktree = (path: string, over: Partial<WorktreeInfo> = {}): WorktreeInfo => ({
  name: path.split('/').at(-1)!,
  path,
  branch: 'main',
  head: '1234567890abcdef',
  is_root: false,
  missing: false,
  dirty: false,
  changed: 0,
  ...over,
})
const ROOT = worktree('/work/p1', { is_root: true })
const FEATURE = worktree('/work/feature', { name: 'feature', branch: 'feature' })

const changesData = (head: string): GitViewChanges => ({
  branch: 'main',
  head,
  conflicts: [],
  staged: [{ path: 'src/app.ts', orig_path: null, status: 'added' }],
  unstaged: [{ path: 'src/app.ts', orig_path: null, status: 'modified' }],
  untracked: [{ path: 'notes.txt', orig_path: null, status: 'untracked' }],
  ahead: null,
})

let qc: QueryClient
const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>
const renderEditor = (checkoutPath = '/work/p1', proj: Project | null = project()) =>
  render(<EditorWorkspace project={proj} projectId="p1" checkoutPath={checkoutPath} />, { wrapper })

/** A change row's select button; the row's Stage/Unstage/Discard buttons share the path in their labels. */
const changeRow = (region: HTMLElement, path: string) => {
  const button = within(region)
    .getAllByRole('button')
    .find((item) => item.hasAttribute('data-nav-item') && item.textContent?.includes(path))
  if (!button) throw new Error(`no change row for ${path}`)
  return button
}

beforeEach(() => {
  vi.useRealTimers()
  qc = makeQueryClient()
  Object.values(service).forEach((fn) => fn.mockReset().mockResolvedValue(undefined))
  Object.values(gitWrite).forEach((fn) => fn.mockReset().mockResolvedValue(undefined))
  Object.values(acp).forEach((fn) => fn.mockReset().mockResolvedValue(undefined))
  settings.get.mockReset().mockResolvedValue(null)
  settings.set.mockReset().mockResolvedValue(undefined)
  gitWrite.commit.mockResolvedValue({ head: 'def5678', branch: 'main' })
  gitWrite.push.mockResolvedValue({ summary: 'pushed main' })
  acp.agents.mockResolvedValue([])
  acp.status.mockResolvedValue({})
  service.changes.mockImplementation((_p: string, co: string) => Promise.resolve(changesData(co)))
  service.diff.mockImplementation((_p: string, _co: string, req: { layer: string; path?: string; commit?: string }) =>
    Promise.resolve({ label: `${req.layer} diff`, patch: `patch ${req.path ?? req.commit}`, truncated: false, files: [] }),
  )
  service.history.mockResolvedValue({
    commits: [
      { id: 'aaa111', parents: ['bbb222'], refs: ['main'], head: true, author: 'Ada', time: 1, subject: 'second commit' },
      { id: 'bbb222', parents: [], refs: [], head: false, author: 'Bob', time: 1, subject: 'first commit' },
    ],
    next_skip: null,
  })
  service.tree.mockImplementation((_p: string, _co: string, dir: string) =>
    Promise.resolve({
      entries:
        dir === ''
          ? [
              { path: 'src', name: 'src', kind: 'dir' as const, ignored: false },
              { path: 'README.md', name: 'README.md', kind: 'file' as const, ignored: false },
            ]
          : [{ path: 'src/app.ts', name: 'app.ts', kind: 'file' as const, ignored: false }],
    }),
  )
  service.file.mockResolvedValue({ path: 'src/app.ts', kind: 'text', text: 'line one\nline two', truncated: false, size: 17 })
  service.fileList.mockResolvedValue(['src/app.ts', 'README.md', 'docs/guide.md'])
  service.search.mockResolvedValue({ matches: [], truncated: false })
  service.stashes.mockResolvedValue([{ index: 0, id: 'stashoid', message: 'saved work', branch: 'main', time: 1 }])
  service.stashDiff.mockResolvedValue({ label: 'stash@{0}', patch: 'stash patch', truncated: false, files: [{ path: 'f.txt', orig_path: null, status: 'modified' as const }] })
  worktrees.list.mockReset().mockResolvedValue({ repo: true, worktrees: [ROOT, FEATURE] })
  useUi.getState().reset()
  useUi.getState().setActiveView('editor')
})

const waitForSearchDebounce = () => act(async () => {
  await new Promise((resolve) => window.setTimeout(resolve, 270))
})

describe('shell', () => {
  it('opens on the Changes section and identifies project, checkout, and branch', async () => {
    renderEditor()
    await waitFor(() => expect(service.changes).toHaveBeenCalledWith('p1', '/work/p1'))
    expect(screen.getByRole('tab', { name: 'Changes' }).getAttribute('aria-selected')).toBe('true')
    const header = screen.getByRole('tablist', { name: 'Editor sections' }).parentElement!
    expect(within(header).getByText('name-p1')).toBeTruthy()
    expect(within(header).getByText('/work/p1')).toBeTruthy()
    expect(within(header).getByText('main')).toBeTruthy()
  })

  it('returns to Terminal from the view switch', async () => {
    renderEditor()
    await screen.findByRole('tab', { name: 'Changes' })
    fireEvent.click(screen.getByRole('button', { name: 'Terminal' }))
    expect(useUi.getState().activeView).toBe('terminal')
  })

  it('shows the unsupported state for a non-Git project with a way back', async () => {
    worktrees.list.mockResolvedValue({ repo: false, worktrees: [] })
    renderEditor()
    expect(await screen.findByText('Editor needs a Git checkout')).toBeTruthy()
    expect(service.changes).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Back to Terminal' }))
    expect(useUi.getState().activeView).toBe('terminal')
  })

  it('shows the unsupported state for an unregistered workspace', async () => {
    renderEditor('/elsewhere', null)
    expect(await screen.findByText('Editor needs a Git checkout')).toBeTruthy()
    expect(worktrees.list).not.toHaveBeenCalled()
  })

  it('shows the missing-checkout state and runs no Git reads', async () => {
    worktrees.list.mockResolvedValue({ repo: true, worktrees: [ROOT, { ...FEATURE, missing: true }] })
    renderEditor('/work/feature')
    expect(await screen.findByText('This checkout is missing')).toBeTruthy()
    expect(service.changes).not.toHaveBeenCalled()
  })

  it('keeps the Editor visible and shows a stale notice when worktree refresh fails', async () => {
    renderEditor()
    expect(await screen.findByRole('tab', { name: 'Changes' })).toBeTruthy()
    worktrees.list.mockRejectedValue(new Error('worktree refresh failed'))

    await qc.invalidateQueries({ queryKey: queryKeys.worktrees('p1') })

    expect(await screen.findByText(/Refresh failed — showing the last result/)).toBeTruthy()
    expect(screen.getByRole('tab', { name: 'Changes' })).toBeTruthy()
    expect(await screen.findByRole('region', { name: 'Staged files' })).toBeTruthy()
  })

  it('keeps Editor active and re-scopes the data when the checkout changes', async () => {
    const { rerender } = renderEditor('/work/p1')
    await waitFor(() => expect(service.diff).toHaveBeenCalledWith('p1', '/work/p1', { layer: 'staged', path: 'src/app.ts' }))

    rerender(<EditorWorkspace project={project()} projectId="p1" checkoutPath="/work/feature" />)
    await waitFor(() => expect(service.changes).toHaveBeenCalledWith('p1', '/work/feature'))
    await waitFor(() => expect(service.diff).toHaveBeenCalledWith('p1', '/work/feature', { layer: 'staged', path: 'src/app.ts' }))
    expect(useUi.getState().activeView).toBe('editor')
  })

  it('shares one resizable list width across the Editor sections', async () => {
    renderEditor()
    const separator = await screen.findByRole('separator', { name: 'Resize list and detail panes' })
    expect(separator.getAttribute('aria-valuenow')).toBe('288')

    fireEvent.keyDown(separator, { key: 'ArrowLeft' })
    await waitFor(() => expect(useUi.getState().panelWidths.editorList).toBe(272))

    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    await screen.findByRole('button', { name: 'src' })
    expect(screen.getByRole('separator', { name: 'Resize list and detail panes' }).getAttribute('aria-valuenow')).toBe('272')
  })
})

describe('sections', () => {
  it('groups Changes with counts and loads the selected file diff for its layer', async () => {
    renderEditor()
    // The file appears in both the staged and unstaged groups.
    const staged = await screen.findByRole('region', { name: 'Staged files' })
    const unstaged = await screen.findByRole('region', { name: 'Unstaged files' })
    expect(changeRow(staged, 'src/app.ts')).toBeTruthy()
    expect(changeRow(unstaged, 'src/app.ts')).toBeTruthy()
    // The first change is selected by default (staged layer).
    await waitFor(() => expect(service.diff).toHaveBeenCalledWith('p1', '/work/p1', { layer: 'staged', path: 'src/app.ts' }))
    expect(screen.getByLabelText('0 conflicts')).toBeTruthy()
    expect(screen.getByLabelText('1 staged')).toBeTruthy()
    expect(screen.getByLabelText('1 unstaged')).toBeTruthy()
    expect(screen.getByLabelText('1 untracked')).toBeTruthy()

    fireEvent.click(changeRow(unstaged, 'src/app.ts'))
    await waitFor(() => expect(service.diff).toHaveBeenCalledWith('p1', '/work/p1', { layer: 'unstaged', path: 'src/app.ts' }))
    expect(await screen.findByText('patch src/app.ts')).toBeTruthy()
    expect(screen.getByText('unstaged diff')).toBeTruthy()
  })

  it('switches the Changes diff between the unified and split views', async () => {
    service.diff.mockResolvedValue({
      label: 'staged diff',
      patch: ['--- a/src/app.ts', '+++ b/src/app.ts', '@@ -1,3 +1,3 @@', ' keep', '-old line', '+new line', ' tail'].join('\n'),
      truncated: false,
      files: [],
    })
    renderEditor()
    expect(await screen.findByText('old line')).toBeTruthy()
    // Unified renders one prefixed line per revision.
    expect(screen.getByText('-')).toBeTruthy()
    expect(screen.getByText('+')).toBeTruthy()

    fireEvent.click(within(screen.getByRole('radiogroup', { name: 'Diff view' })).getByRole('radio', { name: 'Split' }))
    expect(useUi.getState().diffView).toBe('split')
    expect(await screen.findByText('old line')).toBeTruthy()
    expect(screen.getByText('new line')).toBeTruthy()
    expect(screen.queryByText('-')).toBeNull()
    expect(document.querySelector('.grid-cols-2')).toBeTruthy()

    fireEvent.click(within(screen.getByRole('radiogroup', { name: 'Diff view' })).getByRole('radio', { name: 'Unified' }))
    expect(useUi.getState().diffView).toBe('unified')
    expect(screen.getByText('-')).toBeTruthy()
    expect(document.querySelector('.grid-cols-2')).toBeNull()
  })

  it('syntax highlights a truncated read-only file preview', async () => {
    service.file.mockResolvedValue({ path: 'src/app.ts', kind: 'text', text: 'const answer: number = 42', truncated: true, size: 1024 * 1024 + 1 })
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    fireEvent.click(await screen.findByRole('button', { name: 'src' }))
    fireEvent.click(await screen.findByRole('button', { name: 'app.ts' }))

    await waitFor(() => expect(document.querySelector('.hljs-keyword')).toBeTruthy())
    expect(document.querySelector('.hljs-keyword')?.textContent).toBe('const')
  })

  it('shows the clean state when nothing changed', async () => {
    service.changes.mockResolvedValue({ branch: 'main', head: 'abc', conflicts: [], staged: [], unstaged: [], untracked: [], ahead: null })
    renderEditor()
    expect(await screen.findByText('Working tree clean')).toBeTruthy()
    expect(service.diff).not.toHaveBeenCalled()
  })

  it('navigates the Files tree lazily and opens the selected file for editing', async () => {
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    fireEvent.click(await screen.findByRole('button', { name: 'src' }))
    fireEvent.click(await screen.findByRole('button', { name: 'app.ts' }))

    await waitFor(() => expect(service.file).toHaveBeenCalledWith('p1', '/work/p1', 'src/app.ts'))
    const editor = await screen.findByRole('textbox', { name: 'Edit src/app.ts' }) as HTMLTextAreaElement
    expect(editor.value).toBe('line one\nline two')
    expect(service.tree).toHaveBeenCalledWith('p1', '/work/p1', '')
    expect(service.tree).toHaveBeenCalledWith('p1', '/work/p1', 'src')
  })

  it('switches sections with number keys and focuses the active filter with Cmd+F', async () => {
    renderEditor()
    await screen.findByRole('region', { name: 'Staged files' })

    fireEvent.keyDown(window, { key: '4' })
    expect(screen.getByRole('tab', { name: 'Stashes' }).getAttribute('aria-selected')).toBe('true')
    expect(await screen.findByLabelText('Filter stashes')).toBeTruthy()

    fireEvent.keyDown(window, { key: '2' })
    expect(screen.getByRole('tab', { name: 'Changes' }).getAttribute('aria-selected')).toBe('true')
    await screen.findByLabelText('Filter changed files')

    fireEvent.keyDown(window, { key: 'f', metaKey: true })
    expect(document.activeElement).toBe(screen.getByLabelText('Filter changed files'))
  })

  it('leaves Cmd+F inside CodeMirror to its search panel and focuses the Files filter from the tree', async () => {
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    const filter = await screen.findByLabelText('Filter files')
    const editor = document.createElement('div')
    editor.className = 'cm-editor'
    editor.tabIndex = 0
    document.body.append(editor)
    editor.focus()
    const event = new KeyboardEvent('keydown', { key: 'f', metaKey: true, bubbles: true, cancelable: true })
    editor.dispatchEvent(event)
    expect(event.defaultPrevented).toBe(false)
    expect(document.activeElement).toBe(editor)

    fireEvent.keyDown(await screen.findByRole('button', { name: 'src' }), { key: 'f', metaKey: true })
    expect(document.activeElement).toBe(filter)
    editor.remove()
  })

  it('moves between list rows with j and k', async () => {
    renderEditor()
    const staged = await screen.findByRole('region', { name: 'Staged files' })
    const first = changeRow(staged, 'src/app.ts')

    fireEvent.keyDown(window, { key: 'j' })
    expect(document.activeElement).toBe(first)

    fireEvent.keyDown(window, { key: 'j' })
    expect(document.activeElement).not.toBe(first)

    fireEvent.keyDown(window, { key: 'k' })
    expect(document.activeElement).toBe(first)
  })

  it('filters the Files tree by path and opens a match for editing', async () => {
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    fireEvent.change(await screen.findByLabelText('Filter files'), { target: { value: 'readme' } })

    await waitFor(() => expect(service.fileList).toHaveBeenCalledWith('p1', '/work/p1'))
    const match = await screen.findByRole('button', { name: /README\.md/ })
    expect(screen.queryByRole('button', { name: /docs\/guide\.md/ })).toBeNull()
    fireEvent.click(match)
    await waitFor(() => expect(service.file).toHaveBeenCalledWith('p1', '/work/p1', 'README.md'))
  })

  it('debounces content search, reveals a result in the open file, and restores name filtering', async () => {
    service.search.mockResolvedValue({
      matches: [{ path: 'src/app.ts', line: 2, text: 'line two' }],
      truncated: true,
    })
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    fireEvent.click(await screen.findByRole('button', { name: 'src' }))
    fireEvent.click(await screen.findByRole('button', { name: 'app.ts' }))
    await screen.findByRole('textbox', { name: 'Edit src/app.ts' })
    const fileCalls = service.file.mock.calls.length

    fireEvent.click(screen.getByRole('radio', { name: 'Content' }))
    const input = screen.getByLabelText('Search file contents')
    fireEvent.change(input, { target: { value: 'a' } })
    await waitForSearchDebounce()
    expect(service.search).not.toHaveBeenCalled()

    fireEvent.change(input, { target: { value: 'read' } })
    await waitForSearchDebounce()
    await waitFor(() => expect(service.search).toHaveBeenCalledWith('p1', '/work/p1', 'read'))
    const result = await screen.findByRole('button', { name: /src\/app\.ts:2.*line two/ })
    expect(screen.getByText('Showing first 500 matches.')).toBeTruthy()
    fireEvent.click(result)

    const editor = screen.getByRole('textbox', { name: 'Edit src/app.ts' })
    expect(editor.getAttribute('data-reveal-line')).toBe('2')
    expect(editor.getAttribute('data-reveal-nonce')).toBe('1')
    expect(service.file).toHaveBeenCalledTimes(fileCalls)

    // Once the editor has moved the cursor the request is dropped, so a remount cannot replay it.
    fireEvent.click(screen.getByTestId('reveal-done'))
    expect(editor.getAttribute('data-reveal-line')).toBeNull()

    fireEvent.click(screen.getByRole('radio', { name: 'Name' }))
    fireEvent.change(screen.getByLabelText('Filter files'), { target: { value: 'readme' } })
    expect(await screen.findByRole('button', { name: 'README.md' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'docs/guide.md' })).toBeNull()
  })

  it('dims the previous content results while the next query loads', async () => {
    let resolveNext: (value: unknown) => void = () => {}
    service.search
      .mockResolvedValueOnce({ matches: [{ path: 'src/app.ts', line: 2, text: 'first answer' }], truncated: false })
      .mockImplementationOnce(() => new Promise((resolve) => { resolveNext = resolve }))
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    await screen.findByLabelText('Filter files')
    fireEvent.click(screen.getByRole('radio', { name: 'Content' }))
    const input = screen.getByLabelText('Search file contents')
    fireEvent.change(input, { target: { value: 'read' } })
    await waitForSearchDebounce()
    const first = await screen.findByRole('button', { name: /first answer/ })
    expect(first.closest('[aria-busy]')?.getAttribute('aria-busy')).toBe('false')

    fireEvent.change(input, { target: { value: 'reads' } })
    await waitForSearchDebounce()
    await waitFor(() => expect(service.search).toHaveBeenCalledTimes(2))
    expect(screen.getByRole('button', { name: /first answer/ }).closest('[aria-busy]')?.getAttribute('aria-busy')).toBe('true')

    await act(async () => {
      resolveNext({ matches: [{ path: 'src/app.ts', line: 5, text: 'second answer' }], truncated: false })
    })
    const second = await screen.findByRole('button', { name: /second answer/ })
    expect(second.closest('[aria-busy]')?.getAttribute('aria-busy')).toBe('false')
  })

  it('routes content-result navigation through the dirty-draft guard', async () => {
    service.file.mockImplementation((_projectId: string, _checkoutPath: string, path: string) =>
      Promise.resolve({ path, kind: 'text' as const, text: 'line one\nline two', truncated: false, size: 17 }),
    )
    service.search.mockResolvedValue({
      matches: [{ path: 'README.md', line: 4, text: 'target text' }],
      truncated: false,
    })
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    await screen.findByLabelText('Filter files')
    fireEvent.click(await screen.findByRole('button', { name: 'src' }))
    fireEvent.click(await screen.findByRole('button', { name: 'app.ts' }))
    const editor = await screen.findByRole('textbox', { name: 'Edit src/app.ts' })
    fireEvent.change(editor, { target: { value: 'edited draft' } })

    fireEvent.click(screen.getByRole('radio', { name: 'Content' }))
    fireEvent.change(screen.getByLabelText('Search file contents'), { target: { value: 'target' } })
    await waitForSearchDebounce()
    const result = await screen.findByRole('button', { name: /README\.md:4.*target text/ })
    fireEvent.click(result)

    expect(useUi.getState().editorGuardOpen).toBe(true)
    expect(screen.getByRole('textbox', { name: 'Edit src/app.ts' })).toBeTruthy()
    expect(service.file).not.toHaveBeenCalledWith('p1', '/work/p1', 'README.md')

    act(() => useUi.getState().continueEditorNavigation(true))
    const nextEditor = await screen.findByRole('textbox', { name: 'Edit README.md' })
    expect(nextEditor.getAttribute('data-reveal-line')).toBe('4')
    expect(useUi.getState().editorDraft).toBeNull()
  })

  describe('go to definition', () => {
    const openApp = async () => {
      service.file.mockImplementation((_projectId: string, _checkoutPath: string, path: string) =>
        Promise.resolve({
          path,
          kind: 'text' as const,
          text: path === 'tsconfig.json' ? '{ // aliases\n "compilerOptions": { "paths": { "@/*": ["./src/*"], }, }, }' : 'line one\nline two',
          truncated: false,
          size: 17,
        }),
      )
      service.fileList.mockResolvedValue(['tsconfig.json', 'src/app.ts', 'src/util.ts', 'src/stores/ui.ts'])
      renderEditor()
      fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
      fireEvent.click(await screen.findByRole('button', { name: 'src' }))
      fireEvent.click(await screen.findByRole('button', { name: 'app.ts' }))
      await screen.findByRole('textbox', { name: 'Edit src/app.ts' })
    }

    it('opens a relative import and an aliased import from tsconfig paths', async () => {
      await openApp()

      fireEvent.click(screen.getByTestId('go-relative'))
      expect(await screen.findByRole('textbox', { name: 'Edit src/util.ts' })).toBeTruthy()
      expect(service.fileList).toHaveBeenCalledWith('p1', '/work/p1')

      fireEvent.click(screen.getByTestId('go-alias'))
      expect(await screen.findByRole('textbox', { name: 'Edit src/stores/ui.ts' })).toBeTruthy()
      expect(service.file).toHaveBeenCalledWith('p1', '/work/p1', 'tsconfig.json')
    })

    it('shows a short notice for a package or a missing file and keeps the current file', async () => {
      await openApp()

      fireEvent.click(screen.getByTestId('go-package'))
      expect((await screen.findByRole('status')).textContent).toBe("No file for 'react' in this checkout")
      expect(screen.getByRole('textbox', { name: 'Edit src/app.ts' })).toBeTruthy()
    })

    it('jumps to the only declaration of a name and reveals its line', async () => {
      service.search.mockResolvedValue({
        matches: [
          { path: 'src/util.ts', line: 7, text: 'export function Foo() {' },
          { path: 'src/app.ts', line: 1, text: '<Foo />' },
        ],
        truncated: false,
      })
      await openApp()

      fireEvent.click(screen.getByTestId('go-name'))
      const editor = await screen.findByRole('textbox', { name: 'Edit src/util.ts' })
      expect(editor.getAttribute('data-reveal-line')).toBe('7')
      expect(service.search).toHaveBeenCalledWith('p1', '/work/p1', 'Foo')
    })

    it('prefers a declaration in the open file', async () => {
      service.search.mockResolvedValue({
        matches: [
          { path: 'src/app.ts', line: 2, text: 'const Foo = () => 1' },
          { path: 'src/util.ts', line: 7, text: 'export function Foo() {' },
        ],
        truncated: false,
      })
      await openApp()

      fireEvent.click(screen.getByTestId('go-name'))
      await waitFor(() => expect(screen.getByRole('textbox', { name: 'Edit src/app.ts' }).getAttribute('data-reveal-line')).toBe('2'))
    })

    it('opens Content search for the name when no single declaration is found', async () => {
      service.search.mockResolvedValue({
        matches: [
          { path: 'src/util.ts', line: 7, text: 'export function Foo() {' },
          { path: 'src/stores/ui.ts', line: 3, text: 'export class Foo {' },
        ],
        truncated: false,
      })
      await openApp()

      fireEvent.click(screen.getByTestId('go-name'))
      expect((await screen.findByLabelText('Search file contents') as HTMLInputElement).value).toBe('Foo')
      expect(screen.getByRole('radio', { name: 'Content' }).getAttribute('aria-checked')).toBe('true')
      expect(screen.getByRole('textbox', { name: 'Edit src/app.ts' })).toBeTruthy()
    })

    it('shows a search failure as a notice', async () => {
      service.search.mockRejectedValue({ code: 'internal', message: 'git exploded' })
      await openApp()

      fireEvent.click(screen.getByTestId('go-name'))
      expect((await screen.findByRole('status')).textContent).toContain('git exploded')
    })

    it('drops a slow lookup once a newer click has run', async () => {
      let resolveSearch!: (value: unknown) => void
      service.search.mockReturnValue(new Promise((resolve) => { resolveSearch = resolve }))
      await openApp()

      fireEvent.click(screen.getByTestId('go-name')) // search still pending
      fireEvent.click(screen.getByTestId('go-relative'))
      await screen.findByRole('textbox', { name: 'Edit src/util.ts' })

      await act(async () => {
        resolveSearch({ matches: [{ path: 'src/stores/ui.ts', line: 3, text: 'export class Foo {' }], truncated: false })
      })
      expect(screen.getByRole('textbox', { name: 'Edit src/util.ts' })).toBeTruthy()
      expect(screen.queryByRole('textbox', { name: 'Edit src/stores/ui.ts' })).toBeNull()
    })

    it('goes through the dirty-draft guard before leaving an edited file', async () => {
      await openApp()
      fireEvent.change(screen.getByRole('textbox', { name: 'Edit src/app.ts' }), { target: { value: 'edited draft' } })

      fireEvent.click(screen.getByTestId('go-relative'))
      await waitFor(() => expect(useUi.getState().editorGuardOpen).toBe(true))
      expect(screen.getByRole('textbox', { name: 'Edit src/app.ts' })).toBeTruthy()

      act(() => useUi.getState().continueEditorNavigation(true))
      expect(await screen.findByRole('textbox', { name: 'Edit src/util.ts' })).toBeTruthy()
    })
  })

  it('shows the empty state and an ErrorPane for content-search outcomes', async () => {
    service.search
      .mockResolvedValueOnce({ matches: [], truncated: false })
      .mockRejectedValueOnce(new Error('search failed'))
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    await screen.findByLabelText('Filter files')
    fireEvent.click(screen.getByRole('radio', { name: 'Content' }))
    const input = screen.getByLabelText('Search file contents')
    fireEvent.change(input, { target: { value: 'empty' } })
    await waitForSearchDebounce()
    expect(await screen.findByText('No matches.')).toBeTruthy()

    fireEvent.change(input, { target: { value: 'error' } })
    await waitForSearchDebounce()
    expect(await screen.findByText('search failed')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Retry' })).toBeTruthy()
  })

  it('opens Markdown source for editing and allows switching to preview', async () => {
    service.file.mockImplementation((_p: string, _co: string, filePath: string) =>
      Promise.resolve({
        path: filePath,
        kind: 'text' as const,
        text: filePath === 'README.md' ? '# Read me\n\nHello **world**.' : 'line one\nline two',
        truncated: false,
        size: 20,
      }),
    )
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    fireEvent.click(await screen.findByRole('button', { name: /README\.md/ }))

    const editor = await screen.findByRole('textbox', { name: 'Edit README.md' }) as HTMLTextAreaElement
    expect(editor.value).toBe('# Read me\n\nHello **world**.')
    const view = screen.getByRole('radiogroup', { name: 'Markdown view' })
    expect(within(view).getByRole('radio', { name: 'Raw' }).getAttribute('aria-checked')).toBe('true')

    fireEvent.click(within(view).getByRole('radio', { name: 'Preview' }))

    expect(await screen.findByRole('heading', { level: 1, name: 'Read me' })).toBeTruthy()
    expect(within(view).getByRole('radio', { name: 'Preview' }).getAttribute('aria-checked')).toBe('true')
  })

  it('keeps other text files editable without a Markdown toggle', async () => {
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    fireEvent.click(await screen.findByRole('button', { name: 'src' }))
    fireEvent.click(await screen.findByRole('button', { name: 'app.ts' }))

    const editor = await screen.findByRole('textbox', { name: 'Edit src/app.ts' }) as HTMLTextAreaElement
    expect(editor.value).toBe('line one\nline two')
    expect(screen.queryByRole('radiogroup', { name: 'Markdown view' })).toBeNull()
  })

  it('passes the persisted Vim setting to the editor', async () => {
    settings.get.mockResolvedValue('1')
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    fireEvent.click(await screen.findByRole('button', { name: 'src' }))
    fireEvent.click(await screen.findByRole('button', { name: 'app.ts' }))

    expect((await screen.findByRole('textbox', { name: 'Edit src/app.ts' })).getAttribute('data-vim')).toBe('true')
  })

  it('filters changed files by path across the groups', async () => {
    renderEditor()
    const staged = await screen.findByRole('region', { name: 'Staged files' })
    expect(changeRow(staged, 'src/app.ts')).toBeTruthy()

    fireEvent.change(screen.getByLabelText('Filter changed files'), { target: { value: 'notes' } })

    expect(within(screen.getByRole('region', { name: 'Staged files' })).queryByRole('button', { name: /src\/app\.ts/ })).toBeNull()
    expect(changeRow(screen.getByRole('region', { name: 'Untracked files' }), 'notes.txt')).toBeTruthy()
    expect(screen.getByLabelText('0 staged')).toBeTruthy()
  })

  it('filters commits by subject, author, or id', async () => {
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'History' }))
    expect(await screen.findByRole('button', { name: /second commit/ })).toBeTruthy()

    fireEvent.change(screen.getByLabelText('Filter commits'), { target: { value: 'bob' } })

    expect(screen.queryByRole('button', { name: /second commit/ })).toBeNull()
    expect(screen.getByRole('button', { name: /first commit/ })).toBeTruthy()
  })

  it('keeps cached file tree and editor content visible with stale notices after refresh failures', async () => {
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    expect(await screen.findByRole('button', { name: 'src' })).toBeTruthy()
    service.tree.mockRejectedValueOnce(new Error('tree refresh failed'))

    await qc.invalidateQueries({ queryKey: queryKeys.gitTree('p1', '/work/p1', '') })

    expect(await screen.findByText(/tree refresh failed/)).toBeTruthy()
    expect(screen.getByRole('button', { name: 'src' })).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'src' }))
    fireEvent.click(await screen.findByRole('button', { name: 'app.ts' }))
    const editor = await screen.findByRole('textbox', { name: 'Edit src/app.ts' }) as HTMLTextAreaElement
    expect(editor.value).toBe('line one\nline two')
    service.file.mockRejectedValueOnce(new Error('preview refresh failed'))

    await qc.invalidateQueries({ queryKey: queryKeys.gitFile('p1', '/work/p1', 'src/app.ts') })

    expect(await screen.findByText(/preview refresh failed/)).toBeTruthy()
    expect((screen.getByRole('textbox', { name: 'Edit src/app.ts' }) as HTMLTextAreaElement).value).toBe('line one\nline two')
  })

  it('links a changed file from Files to its Changes diff', async () => {
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    fireEvent.click(await screen.findByRole('button', { name: 'src' }))
    fireEvent.click(await screen.findByRole('button', { name: 'app.ts' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Show changes' }))

    expect(screen.getByRole('tab', { name: 'Changes' }).getAttribute('aria-selected')).toBe('true')
    await waitFor(() => expect(service.diff).toHaveBeenCalledWith('p1', '/work/p1', { layer: 'staged', path: 'src/app.ts' }))
  })

  it('selects the HEAD commit in History and shows its patch and files', async () => {
    service.diff.mockResolvedValue({ label: 'commit aaa111', patch: 'commit patch', truncated: false, files: [{ path: 'f.txt', orig_path: null, status: 'modified' as const }] })
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'History' }))

    await waitFor(() => expect(service.diff).toHaveBeenCalledWith('p1', '/work/p1', { layer: 'commit', commit: 'aaa111' }))
    expect(await screen.findByText('second commit')).toBeTruthy()
    expect(screen.getByText('first commit')).toBeTruthy()
    expect(await screen.findByText('commit patch')).toBeTruthy()
    expect(within(screen.getByRole('list', { name: 'Changed files' })).getByText('f.txt')).toBeTruthy()
    const headRow = screen.getByRole('button', { name: /second commit/ })
    expect(within(headRow).getByText('HEAD')).toBeTruthy()
    expect(within(headRow).getByText('main')).toBeTruthy()
  })

  it('shows an empty history state', async () => {
    service.history.mockResolvedValue({ commits: [], next_skip: null })
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'History' }))
    expect(await screen.findByText('No commits yet')).toBeTruthy()
  })

  it('labels Stashes as repository-wide and loads the newest stash patch', async () => {
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'Stashes' }))
    expect(await screen.findByText(/Shared across all worktrees/)).toBeTruthy()
    await waitFor(() => expect(service.stashDiff).toHaveBeenCalledWith('p1', '/work/p1', 'stashoid'))
    expect(await screen.findByText('stash patch')).toBeTruthy()
    expect(screen.getByText('branch: main')).toBeTruthy()
  })

  it('keeps the stash selection when the checkout changes within the project', async () => {
    const { rerender } = renderEditor('/work/p1')
    fireEvent.click(screen.getByRole('tab', { name: 'Stashes' }))
    await waitFor(() => expect(service.stashDiff).toHaveBeenCalledWith('p1', '/work/p1', 'stashoid'))

    rerender(<EditorWorkspace project={project()} projectId="p1" checkoutPath="/work/feature" />)
    // The list is keyed by project — the same shared stack is reused as-is and
    // the selection and its cached patch survive the checkout switch.
    expect(service.stashes).toHaveBeenCalledTimes(1)
    expect((await screen.findAllByText('stash@{0}')).length).toBeGreaterThan(0)
    expect(await screen.findByText('stash patch')).toBeTruthy()
  })

  it('shows the empty stash state', async () => {
    service.stashes.mockResolvedValue([])
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'Stashes' }))
    expect(await screen.findByText('No stashes in this repository.')).toBeTruthy()
  })

  it('offers a retry on a failed first load', async () => {
    service.changes.mockRejectedValue({ code: 'internal', message: 'git blew up', hint: null })
    renderEditor()
    expect((await screen.findByText('git blew up')).closest('[role="alert"]')).toBeTruthy()
    service.changes.mockResolvedValue(changesData('/work/p1'))
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    await waitFor(() => expect(service.changes).toHaveBeenCalledTimes(2))
    expect((await screen.findAllByText('src/app.ts')).length).toBeGreaterThan(0)
  })
})

describe('change list writes', () => {
  it('stages one file from its row action', async () => {
    renderEditor()
    const unstaged = await screen.findByRole('region', { name: 'Unstaged files' })
    fireEvent.click(within(unstaged).getByRole('button', { name: 'Stage src/app.ts' }))
    await waitFor(() => expect(gitWrite.stage).toHaveBeenCalledWith('p1', '/work/p1', ['src/app.ts']))
  })

  it('unstages one file from its row action', async () => {
    renderEditor()
    const staged = await screen.findByRole('region', { name: 'Staged files' })
    fireEvent.click(within(staged).getByRole('button', { name: 'Unstage src/app.ts' }))
    await waitFor(() => expect(gitWrite.unstage).toHaveBeenCalledWith('p1', '/work/p1', ['src/app.ts']))
  })

  it('stages every unstaged path from its group header', async () => {
    renderEditor()
    const unstaged = await screen.findByRole('region', { name: 'Unstaged files' })
    fireEvent.click(within(unstaged).getByRole('button', { name: 'Stage all' }))
    await waitFor(() => expect(gitWrite.stage).toHaveBeenCalledWith('p1', '/work/p1', ['src/app.ts']))
    expect(gitWrite.stage).toHaveBeenCalledTimes(1)
  })

  it('unstages every staged path with null', async () => {
    renderEditor()
    const staged = await screen.findByRole('region', { name: 'Staged files' })
    fireEvent.click(within(staged).getByRole('button', { name: 'Unstage all' }))
    await waitFor(() => expect(gitWrite.unstage).toHaveBeenCalledWith('p1', '/work/p1', null))
  })

  it('asks before discarding and does nothing on Cancel', async () => {
    renderEditor()
    const untracked = await screen.findByRole('region', { name: 'Untracked files' })
    fireEvent.click(within(untracked).getByRole('button', { name: 'Discard notes.txt' }))

    const dialog = await screen.findByRole('dialog')
    expect(dialog.textContent).toContain('notes.txt')
    expect(dialog.textContent).toContain('This cannot be undone.')

    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    expect(gitWrite.discard).not.toHaveBeenCalled()
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
  })

  it('confirms discard with the row\'s layer', async () => {
    renderEditor()
    const unstaged = await screen.findByRole('region', { name: 'Unstaged files' })
    fireEvent.click(within(unstaged).getByRole('button', { name: 'Discard src/app.ts' }))
    const dialog = await screen.findByRole('dialog')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Discard' }))
    await waitFor(() => expect(gitWrite.discard).toHaveBeenCalledWith('p1', '/work/p1', 'unstaged', 'src/app.ts'))
  })

  it('keeps the discard dialog open and shows its error on failure', async () => {
    gitWrite.discard.mockRejectedValue({ code: 'internal', message: 'file is locked', hint: null })
    renderEditor()
    const untracked = await screen.findByRole('region', { name: 'Untracked files' })
    fireEvent.click(within(untracked).getByRole('button', { name: 'Discard notes.txt' }))
    const dialog = await screen.findByRole('dialog')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Discard' }))
    await waitFor(() => expect(within(dialog).getByText('file is locked')).toBeTruthy())
    expect(screen.getByRole('dialog')).toBeTruthy()
  })

  it('shows a write failure above the list', async () => {
    gitWrite.stage.mockRejectedValue({ code: 'internal', message: 'index.lock is held', hint: null })
    renderEditor()
    const unstaged = await screen.findByRole('region', { name: 'Unstaged files' })
    fireEvent.click(within(unstaged).getByRole('button', { name: 'Stage src/app.ts' }))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('index.lock is held')
  })

  it('disables the row actions while a write is pending', async () => {
    let resolveStage!: () => void
    gitWrite.stage.mockImplementation(
      () => new Promise<void>((resolve) => { resolveStage = resolve }),
    )
    renderEditor()
    const unstaged = await screen.findByRole('region', { name: 'Unstaged files' })
    fireEvent.click(within(unstaged).getByRole('button', { name: 'Stage src/app.ts' }))
    await waitFor(() =>
      expect((within(unstaged).getByRole('button', { name: 'Stage src/app.ts' }) as HTMLButtonElement).disabled).toBe(true),
    )
    await act(async () => { resolveStage() })
  })

  it('keeps row selection working next to the action buttons', async () => {
    renderEditor()
    const staged = await screen.findByRole('region', { name: 'Staged files' })
    const unstaged = await screen.findByRole('region', { name: 'Unstaged files' })
    await waitFor(() =>
      expect(service.diff).toHaveBeenCalledWith('p1', '/work/p1', { layer: 'staged', path: 'src/app.ts' }),
    )

    fireEvent.click(within(unstaged).getByRole('button', { name: 'Stage src/app.ts' }))
    await waitFor(() => expect(gitWrite.stage).toHaveBeenCalledWith('p1', '/work/p1', ['src/app.ts']))
    expect(changeRow(staged, 'src/app.ts').getAttribute('aria-pressed')).toBe('true')
    expect(changeRow(unstaged, 'src/app.ts').getAttribute('aria-pressed')).toBe('false')

    fireEvent.click(changeRow(unstaged, 'src/app.ts'))
    await waitFor(() =>
      expect(service.diff).toHaveBeenLastCalledWith('p1', '/work/p1', { layer: 'unstaged', path: 'src/app.ts' }),
    )
  })
})
