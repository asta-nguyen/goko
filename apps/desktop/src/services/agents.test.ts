import { agentService } from './agents'

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke }))

beforeEach(() => invoke.mockReset().mockResolvedValue(undefined))

describe('agentService commands', () => {
  it('calls agent_list with no payload', async () => {
    await agentService.list()
    expect(invoke.mock.calls).toEqual([['agent_list', undefined]])
  })
})
