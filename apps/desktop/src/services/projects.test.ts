import { projectService } from './projects'

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke }))

beforeEach(() => invoke.mockReset().mockResolvedValue(undefined))

describe('projectService commands', () => {
  it('calls each command with its payload', async () => {
    await projectService.list()
    await projectService.add('/work/app')
    await projectService.remove('p1')
    expect(invoke.mock.calls).toEqual([
      ['project_list', undefined],
      ['project_add', { path: '/work/app' }],
      ['project_remove', { id: 'p1' }],
    ])
  })

  it('turns a rejected invoke into a BridgeError', async () => {
    invoke.mockRejectedValueOnce({ code: 'storage', message: 'db locked', hint: null })
    await expect(projectService.list()).rejects.toEqual({ code: 'storage', message: 'db locked', hint: null })
  })
})
