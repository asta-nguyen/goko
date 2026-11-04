import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient, queryKeys } from '../../queryClient'
import type { GitViewChanges, Project, WorktreeInfo } from '../../services/types'
import { useUi } from '../../stores/ui'
import { EditorWorkspace } from './EditorWorkspace'

const { service, worktrees } = vi.hoisted(() => ({
  service: {
    changes: vi.fn(),
    diff: vi.fn(),
    history: vi.fn(),
    tree: vi.fn(),
    fileList: vi.fn(),
    file: vi.fn(),
    stashes: vi.fn(),
    stashDiff: vi.fn(),
  },
  worktrees: { list: vi.fn() },
}))
vi.mock('../../services/git-view', () => ({ gitViewService: service }))
vi.mock('../../services/worktrees', () => ({ worktreeService: worktrees }))

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
})

let qc: QueryClient
const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>
const renderEditor = (checkoutPath = '/work/p1', proj: Project | null = project()) =>
  render(<EditorWorkspace project={proj} projectId="p1" checkoutPath={checkoutPath} />, { wrapper })

beforeEach(() => {
  qc = makeQueryClient()
  Object.values(service).forEach((fn) => fn.mockReset().mockResolvedValue(undefined))
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
              { path: 'src', name: 'src', kind: 'dir' as const },
              { path: 'README.md', name: 'README.md', kind: 'file' as const },
            ]
          : [{ path: 'src/app.ts', name: 'app.ts', kind: 'file' as const }],
    }),
  )
  service.file.mockResolvedValue({ path: 'src/app.ts', kind: 'text', text: 'line one\nline two', truncated: false, size: 17 })
  service.fileList.mockResolvedValue(['src/app.ts', 'README.md', 'docs/guide.md'])
  service.stashes.mockResolvedValue([{ index: 0, id: 'stashoid', message: 'saved work', branch: 'main', time: 1 }])
  service.stashDiff.mockResolvedValue({ label: 'stash@{0}', patch: 'stash patch', truncated: false, files: [{ path: 'f.txt', orig_path: null, status: 'modified' as const }] })
  worktrees.list.mockReset().mockResolvedValue({ repo: true, worktrees: [ROOT, FEATURE] })
  useUi.getState().reset()
  useUi.getState().setActiveView('editor')
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
    expect(within(staged).getByRole('button', { name: /src\/app\.ts/ })).toBeTruthy()
    expect(within(unstaged).getByRole('button', { name: /src\/app\.ts/ })).toBeTruthy()
    // The first change is selected by default (staged layer).
    await waitFor(() => expect(service.diff).toHaveBeenCalledWith('p1', '/work/p1', { layer: 'staged', path: 'src/app.ts' }))
    expect(screen.getByLabelText('0 conflicts')).toBeTruthy()
    expect(screen.getByLabelText('1 staged')).toBeTruthy()
    expect(screen.getByLabelText('1 unstaged')).toBeTruthy()
    expect(screen.getByLabelText('1 untracked')).toBeTruthy()

    fireEvent.click(within(unstaged).getByRole('button', { name: /src\/app\.ts/ }))
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
    service.changes.mockResolvedValue({ branch: 'main', head: 'abc', conflicts: [], staged: [], unstaged: [], untracked: [] })
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

  it('moves between list rows with j and k', async () => {
    renderEditor()
    const staged = await screen.findByRole('region', { name: 'Staged files' })
    const first = within(staged).getByRole('button', { name: /src\/app\.ts/ })

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

  it('filters changed files by path across the groups', async () => {
    renderEditor()
    const staged = await screen.findByRole('region', { name: 'Staged files' })
    expect(within(staged).getByRole('button', { name: /src\/app\.ts/ })).toBeTruthy()

    fireEvent.change(screen.getByLabelText('Filter changed files'), { target: { value: 'notes' } })

    expect(within(screen.getByRole('region', { name: 'Staged files' })).queryByRole('button', { name: /src\/app\.ts/ })).toBeNull()
    expect(within(screen.getByRole('region', { name: 'Untracked files' })).getByRole('button', { name: /notes\.txt/ })).toBeTruthy()
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
