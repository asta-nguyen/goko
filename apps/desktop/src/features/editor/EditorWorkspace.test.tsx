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

  it('shows the clean state when nothing changed', async () => {
    service.changes.mockResolvedValue({ branch: 'main', head: 'abc', conflicts: [], staged: [], unstaged: [], untracked: [] })
    renderEditor()
    expect(await screen.findByText('Working tree clean')).toBeTruthy()
    expect(service.diff).not.toHaveBeenCalled()
  })

  it('navigates the Files tree lazily and previews the file with line numbers', async () => {
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    fireEvent.click(await screen.findByRole('button', { name: 'src' }))
    fireEvent.click(await screen.findByRole('button', { name: 'app.ts' }))

    await waitFor(() => expect(service.file).toHaveBeenCalledWith('p1', '/work/p1', 'src/app.ts'))
    expect(await screen.findByText('line one')).toBeTruthy()
    expect(screen.getByText('2')).toBeTruthy()
    expect(service.tree).toHaveBeenCalledWith('p1', '/work/p1', '')
    expect(service.tree).toHaveBeenCalledWith('p1', '/work/p1', 'src')
  })

  it('keeps cached file tree and preview visible with stale notices after refresh failures', async () => {
    renderEditor()
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    expect(await screen.findByRole('button', { name: 'src' })).toBeTruthy()
    service.tree.mockRejectedValueOnce(new Error('tree refresh failed'))

    await qc.invalidateQueries({ queryKey: queryKeys.gitTree('p1', '/work/p1', '') })

    expect(await screen.findByText(/tree refresh failed/)).toBeTruthy()
    expect(screen.getByRole('button', { name: 'src' })).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'src' }))
    fireEvent.click(await screen.findByRole('button', { name: 'app.ts' }))
    expect(await screen.findByText('line one')).toBeTruthy()
    service.file.mockRejectedValueOnce(new Error('preview refresh failed'))

    await qc.invalidateQueries({ queryKey: queryKeys.gitFile('p1', '/work/p1', 'src/app.ts') })

    expect(await screen.findByText(/preview refresh failed/)).toBeTruthy()
    expect(screen.getByText('line one')).toBeTruthy()
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
