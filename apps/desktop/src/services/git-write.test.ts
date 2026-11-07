import { gitWriteService } from './git-write'

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke }))

beforeEach(() => {
  invoke.mockReset().mockResolvedValue(undefined)
})

describe('gitWriteService commands', () => {
  it('calls each command with its payload', async () => {
    await gitWriteService.stage('p1', '/co', ['a.txt'])
    await gitWriteService.stage('p1', '/co', null)
    await gitWriteService.unstage('p1', '/co', ['a.txt'])
    await gitWriteService.unstage('p1', '/co', null)
    await gitWriteService.discard('p1', '/co', 'unstaged', 'a.txt')
    await gitWriteService.discard('p1', '/co', 'untracked', 'b.txt')
    await gitWriteService.commit('p1', '/co', 'a message')
    await gitWriteService.push('p1', '/co')
    expect(invoke.mock.calls).toEqual([
      ['git_write_stage', { projectId: 'p1', checkoutPath: '/co', paths: ['a.txt'] }],
      ['git_write_stage', { projectId: 'p1', checkoutPath: '/co', paths: null }],
      ['git_write_unstage', { projectId: 'p1', checkoutPath: '/co', paths: ['a.txt'] }],
      ['git_write_unstage', { projectId: 'p1', checkoutPath: '/co', paths: null }],
      ['git_write_discard', { projectId: 'p1', checkoutPath: '/co', layer: 'unstaged', path: 'a.txt' }],
      ['git_write_discard', { projectId: 'p1', checkoutPath: '/co', layer: 'untracked', path: 'b.txt' }],
      ['git_write_commit', { projectId: 'p1', checkoutPath: '/co', message: 'a message' }],
      ['git_write_push', { projectId: 'p1', checkoutPath: '/co' }],
    ])
  })

  it('turns a rejected invoke into a BridgeError', async () => {
    invoke.mockRejectedValueOnce({ code: 'invalid_argument', message: 'nothing is staged to commit', hint: null })
    await expect(gitWriteService.commit('p', '/c', 'a message')).rejects.toEqual({
      code: 'invalid_argument',
      message: 'nothing is staged to commit',
      hint: null,
    })
  })
})
