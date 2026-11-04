import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient, queryKeys } from '../../queryClient'
import type { GitViewChanges } from '../../services/types'
import {
  useGitChanges,
  useGitDiff,
  useGitFile,
  useGitHistory,
  useGitSearch,
  useGitStashDiff,
  useGitStashes,
  useGitTree,
  useEditorVim,
} from './queries'

const service = vi.hoisted(() => ({
  changes: vi.fn(),
  diff: vi.fn(),
  history: vi.fn(),
  tree: vi.fn(),
  file: vi.fn(),
  search: vi.fn(),
  stashes: vi.fn(),
  stashDiff: vi.fn(),
}))
const settings = vi.hoisted(() => ({ get: vi.fn(), set: vi.fn() }))
vi.mock('../../services/git-view', () => ({ gitViewService: service }))
vi.mock('../../services/settings', () => ({ settingsService: settings }))

const changes = (head: string): GitViewChanges => ({
  branch: 'main',
  head,
  conflicts: [],
  staged: [],
  unstaged: [{ path: `${head}.txt`, orig_path: null, status: 'modified' }],
  untracked: [],
})

let qc: QueryClient
const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={qc}>{children}</QueryClientProvider>
)

beforeEach(() => {
  vi.useRealTimers()
  qc = makeQueryClient()
  Object.values(service).forEach((fn) => fn.mockReset().mockResolvedValue(undefined))
  settings.get.mockReset().mockResolvedValue(null)
  settings.set.mockReset().mockResolvedValue(undefined)
})

describe('Vim setting', () => {
  it('persists the enabled value and reads it back through the query cache', async () => {
    const { result } = renderHook(() => useEditorVim(), { wrapper })
    await waitFor(() => expect(settings.get).toHaveBeenCalledWith('editor.vim'))
    expect(result.current[0]).toBe(false)

    act(() => result.current[1](true))
    await waitFor(() => expect(settings.set).toHaveBeenLastCalledWith('editor.vim', '1'))
    await waitFor(() => expect(result.current[0]).toBe(true))

    act(() => result.current[1](false))
    await waitFor(() => expect(settings.set).toHaveBeenLastCalledWith('editor.vim', '0'))
    await waitFor(() => expect(result.current[0]).toBe(false))
    expect(settings.set.mock.calls).toEqual([['editor.vim', '1'], ['editor.vim', '0']])
  })

  it('keeps Vim off when reading the setting fails', async () => {
    settings.get.mockRejectedValue(new Error('settings unavailable'))
    const { result } = renderHook(() => useEditorVim(), { wrapper })
    await waitFor(() => expect(result.current[0]).toBe(false))
    expect(result.current[0]).toBe(false)
  })
})

describe('checkout-scoped queries', () => {
  it('keeps each checkout in its own entry and reuses only its own cache', async () => {
    service.changes.mockImplementation((_p: string, co: string) => Promise.resolve(changes(co)))
    const { result, rerender } = renderHook(
      ({ checkout }) => useGitChanges('p1', checkout, true),
      { initialProps: { checkout: '/a' }, wrapper },
    )
    await waitFor(() => expect(result.current.data?.head).toBe('/a'))

    rerender({ checkout: '/b' })
    // The previous checkout's data is never rendered under the new identity.
    expect(result.current.data).toBeUndefined()
    await waitFor(() => expect(result.current.data?.head).toBe('/b'))
    expect(service.changes.mock.calls).toEqual([
      ['p1', '/a'],
      ['p1', '/b'],
    ])

    rerender({ checkout: '/a' })
    // `/a`'s own cached result shows while its fresh fetch runs.
    expect(result.current.data?.head).toBe('/a')
    expect(qc.getQueryData(queryKeys.gitChanges('p1', '/a'))).toEqual(changes('/a'))
  })

  it('scopes diff, history, tree, and file reads to the checkout', async () => {
    service.diff.mockResolvedValue({ label: 'l', patch: 'p', truncated: false, files: [] })
    service.history.mockResolvedValue({ commits: [], next_skip: null })
    service.tree.mockResolvedValue({ entries: [] })
    service.file.mockResolvedValue({ path: 'f', kind: 'text', text: 'x', truncated: false, size: 1 })

    renderHook(() => {
      useGitDiff('p1', '/a', { layer: 'unstaged', path: 'f.txt' }, { enabled: true })
      useGitHistory('p1', '/a', true)
      useGitTree('p1', '/a', '', true)
      useGitFile('p1', '/a', 'f.txt', true)
    }, { wrapper })
    await waitFor(() => expect(service.file).toHaveBeenCalled())

    for (const [key, fn] of [
      ['git-diff', service.diff],
      ['git-history', service.history],
      ['git-tree', service.tree],
      ['git-file', service.file],
    ] as const) {
      expect(qc.getQueryCache().findAll({ queryKey: [key, 'p1', '/a'] })).toHaveLength(1)
      expect(qc.getQueryCache().findAll({ queryKey: [key, 'p1', '/b'] })).toHaveLength(0)
      expect(fn.mock.calls[0][1]).toBe('/a')
    }
  })

  it('runs no read while the Editor view is disabled', async () => {
    renderHook(() => {
      useGitChanges('p1', '/a', false)
      useGitDiff('p1', '/a', { layer: 'staged', path: 'f' }, { enabled: false })
      useGitHistory('p1', '/a', false)
      useGitTree('p1', '/a', '', false)
      useGitFile('p1', '/a', 'f', false)
      useGitSearch('p1', '/a', 'ab', false)
      useGitStashes('p1', '/a', false)
      useGitStashDiff('p1', '/a', 'oid', false)
    }, { wrapper })
    await act(async () => {})
    for (const fn of Object.values(service)) expect(fn).not.toHaveBeenCalled()
  })

  it('rekeys search by query, keeps the previous result while loading, and collects it soon after', async () => {
    const first = { matches: [{ path: 'a.ts', line: 1, text: 'first' }], truncated: false }
    const second = { matches: [{ path: 'b.ts', line: 2, text: 'second' }], truncated: false }
    let resolveSecond!: (result: typeof second) => void
    service.search
      .mockResolvedValueOnce(first)
      .mockImplementationOnce(() => new Promise((resolve) => { resolveSecond = resolve }))
    const { result, rerender } = renderHook(
      ({ query }) => useGitSearch('p1', '/a', query, true),
      { initialProps: { query: 'ab' }, wrapper },
    )
    await waitFor(() => expect(result.current.data).toEqual(first))

    rerender({ query: 'cd' })
    await waitFor(() => expect(service.search).toHaveBeenCalledTimes(2))
    expect(result.current.data).toEqual(first)
    expect(result.current.isPlaceholderData).toBe(true)
    expect(qc.getQueryData(queryKeys.gitSearch('p1', '/a', 'ab'))).toEqual(first)

    await act(async () => resolveSecond(second))
    await waitFor(() => expect(result.current.data).toEqual(second))
    expect(qc.getQueryData(queryKeys.gitSearch('p1', '/a', 'cd'))).toEqual(second)
  })

  it('keeps the last good result behind an error state when a refresh fails', async () => {
    service.changes.mockResolvedValueOnce(changes('/a')).mockRejectedValue({ code: 'internal', message: 'git broke', hint: null })
    const { result } = renderHook(() => useGitChanges('p1', '/a', true), { wrapper })
    await waitFor(() => expect(result.current.data?.head).toBe('/a'))
    // Read the error flags up front so the observer tracks them: without a
    // tracked prop changing, a failed refetch would not re-render the hook.
    expect(result.current.isError).toBe(false)
    expect(result.current.error).toBeNull()

    await act(async () => {
      await result.current.refetch()
    })
    await waitFor(() => expect(result.current.isError).toBe(true))
    expect(result.current.error).toEqual({ code: 'internal', message: 'git broke', hint: null })
    expect(result.current.data?.head).toBe('/a')
  })

  it('waits for a diff selection and re-keys when it changes', async () => {
    service.diff.mockResolvedValue({ label: 'l', patch: 'p', truncated: false, files: [] })
    const { rerender } = renderHook(
      ({ path }) => useGitDiff('p1', '/a', path === null ? null : { layer: 'unstaged', path }),
      { initialProps: { path: null as string | null }, wrapper },
    )
    await act(async () => {})
    expect(service.diff).not.toHaveBeenCalled()

    rerender({ path: 'one.txt' })
    rerender({ path: 'two.txt' })
    await waitFor(() =>
      expect(service.diff.mock.calls).toEqual([
        ['p1', '/a', { layer: 'unstaged', path: 'one.txt' }],
        ['p1', '/a', { layer: 'unstaged', path: 'two.txt' }],
      ]),
    )
  })
})

describe('polling', () => {
  it('polls Changes and Stashes every three seconds only while visible', async () => {
    vi.useFakeTimers()
    service.changes.mockResolvedValue(changes('/a'))
    service.stashes.mockResolvedValue([])
    service.diff.mockResolvedValue({ label: 'l', patch: 'p', truncated: false, files: [] })
    const { rerender } = renderHook(
      ({ visible }) => {
        useGitChanges('p1', '/a', visible)
        useGitDiff('p1', '/a', { layer: 'unstaged', path: 'f' }, { poll: visible })
        useGitStashes('p1', '/a', visible)
      },
      { initialProps: { visible: true }, wrapper },
    )
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(service.changes).toHaveBeenCalledTimes(1)
    expect(service.stashes).toHaveBeenCalledTimes(1)
    expect(service.diff).toHaveBeenCalledTimes(1)

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3_000)
    })
    expect(service.changes).toHaveBeenCalledTimes(2)
    expect(service.stashes).toHaveBeenCalledTimes(2)
    expect(service.diff).toHaveBeenCalledTimes(2)

    // Hiding the section stops every poll.
    rerender({ visible: false })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(9_000)
    })
    expect(service.changes).toHaveBeenCalledTimes(2)
    expect(service.stashes).toHaveBeenCalledTimes(2)
    expect(service.diff).toHaveBeenCalledTimes(2)
  })

  it('does not poll unpinned reads like history or a selected file', async () => {
    vi.useFakeTimers()
    service.history.mockResolvedValue({ commits: [], next_skip: null })
    service.file.mockResolvedValue({ path: 'f', kind: 'text', text: 'x', truncated: false, size: 1 })
    service.stashDiff.mockResolvedValue({ label: 'l', patch: 'p', truncated: false, files: [] })
    renderHook(() => {
      useGitHistory('p1', '/a', true)
      useGitFile('p1', '/a', 'f.txt', true)
      useGitStashDiff('p1', '/a', 'oid', true)
    }, { wrapper })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(9_000)
    })
    expect(service.history).toHaveBeenCalledTimes(1)
    expect(service.file).toHaveBeenCalledTimes(1)
    expect(service.stashDiff).toHaveBeenCalledTimes(1)
  })
})

describe('repository-wide stashes', () => {
  it('shares one list entry across worktrees of the same project', async () => {
    service.stashes.mockResolvedValue([{ index: 0, id: 'oid', message: 'm', branch: 'main', time: 1 }])
    const { rerender } = renderHook(
      ({ checkout }) => useGitStashes('p1', checkout, true),
      { initialProps: { checkout: '/a' }, wrapper },
    )
    await waitFor(() => expect(service.stashes).toHaveBeenCalledWith('p1', '/a'))

    rerender({ checkout: '/b' })
    // One cache entry for the repository — the worktrees share the stack, so
    // `/b` renders the same result under the same key.
    expect(qc.getQueryCache().findAll({ queryKey: ['git-stashes'] })).toHaveLength(1)
    expect(qc.getQueryData(queryKeys.gitStashes('p1'))).toHaveLength(1)
  })

  it('keys a stash patch by its immutable commit id, not the checkout', async () => {
    service.stashDiff.mockResolvedValue({ label: 'stash@{0}', patch: 'p', truncated: false, files: [] })
    const { rerender } = renderHook(
      ({ checkout }) => useGitStashDiff('p1', checkout, 'oid', true),
      { initialProps: { checkout: '/a' }, wrapper },
    )
    await waitFor(() => expect(service.stashDiff).toHaveBeenCalledWith('p1', '/a', 'oid'))
    rerender({ checkout: '/b' })
    // Still one entry: the stash commit id identifies the patch, not the checkout.
    expect(qc.getQueryCache().findAll({ queryKey: ['git-stash-diff'] })).toHaveLength(1)
    expect(qc.getQueryData(queryKeys.gitStashDiff('p1', 'oid'))).toBeTruthy()
  })
})

describe('history paging', () => {
  it('fetches the next page at the returned skip offset', async () => {
    const commit = (id: string) => ({ id, parents: [], refs: [], head: false, author: 'a', time: 1, subject: 's' })
    service.history
      .mockResolvedValueOnce({ commits: [commit('a')], next_skip: 100 })
      .mockResolvedValueOnce({ commits: [commit('b')], next_skip: null })
    const { result } = renderHook(() => useGitHistory('p1', '/a', true), { wrapper })
    await waitFor(() => expect(result.current.data?.pages[0].commits[0].id).toBe('a'))

    await act(async () => {
      await result.current.fetchNextPage()
    })
    expect(service.history.mock.calls[1]).toEqual(['p1', '/a', 100])
    await waitFor(() =>
      expect(result.current.data?.pages.flatMap((page) => page.commits).map((c) => c.id)).toEqual(['a', 'b']),
    )
    expect(result.current.hasNextPage).toBe(false)
  })
})
