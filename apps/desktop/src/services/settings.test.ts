import { settingsService } from './settings'

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke }))

beforeEach(() => invoke.mockReset().mockResolvedValue(undefined))

describe('settingsService commands', () => {
  it('calls each command with its payload', async () => {
    invoke.mockResolvedValueOnce('p1')
    await expect(settingsService.get('active_project_id')).resolves.toBe('p1')
    await settingsService.set('active_project_id', 'p1')
    expect(invoke.mock.calls).toEqual([
      ['settings_get', { key: 'active_project_id' }],
      ['settings_set', { key: 'active_project_id', value: 'p1' }],
    ])
  })

  it('turns a rejected invoke into a BridgeError', async () => {
    invoke.mockRejectedValueOnce({ code: 'storage', message: 'no db', hint: null })
    await expect(settingsService.get('k')).rejects.toMatchObject({ code: 'storage' })
  })
})
