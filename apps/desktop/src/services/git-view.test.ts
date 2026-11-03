import { gitViewService } from './git-view'

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke }))

beforeEach(() => {
  invoke.mockReset().mockResolvedValue(undefined)
})

describe('gitViewService commands', () => {
  it('calls each command with its payload', async () => {
    await gitViewService.changes('p1', '/co')
    await gitViewService.diff('p1', '/co', { layer: 'staged', path: 'a.txt' })
    await gitViewService.diff('p1', '/co', { layer: 'commit', commit: 'abc' })
    await gitViewService.history('p1', '/co', 100)
    await gitViewService.tree('p1', '/co', 'dir')
    await gitViewService.file('p1', '/co', 'dir/f.txt')
    await gitViewService.stashes('p1', '/co')
    await gitViewService.stashDiff('p1', '/co', 'oid')
    expect(invoke.mock.calls).toEqual([
      ['git_view_changes', { projectId: 'p1', checkoutPath: '/co' }],
      ['git_view_diff', { projectId: 'p1', checkoutPath: '/co', layer: 'staged', path: 'a.txt', commit: null }],
      ['git_view_diff', { projectId: 'p1', checkoutPath: '/co', layer: 'commit', path: null, commit: 'abc' }],
      ['git_view_history', { projectId: 'p1', checkoutPath: '/co', skip: 100 }],
      ['git_view_tree', { projectId: 'p1', checkoutPath: '/co', directory: 'dir' }],
      ['git_view_file', { projectId: 'p1', checkoutPath: '/co', path: 'dir/f.txt' }],
      ['git_view_stashes', { projectId: 'p1', checkoutPath: '/co' }],
      ['git_view_stash_diff', { projectId: 'p1', checkoutPath: '/co', stashId: 'oid' }],
    ])
  })

  it('turns a rejected invoke into a BridgeError', async () => {
    invoke.mockRejectedValueOnce({ code: 'invalid_argument', message: 'not a Git worktree', hint: null })
    await expect(gitViewService.changes('p', '/c')).rejects.toEqual({
      code: 'invalid_argument',
      message: 'not a Git worktree',
      hint: null,
    })
  })
})
