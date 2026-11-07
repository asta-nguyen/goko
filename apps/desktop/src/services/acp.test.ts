import { acpService } from './acp'

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke }))

beforeEach(() => {
  invoke.mockReset().mockResolvedValue(undefined)
})

describe('acpService commands', () => {
  it('calls each command with its payload', async () => {
    const overrides = [{ id: 'devin', enabled: false, command: 'devin acp', prompt: '' }]
    await acpService.agents()
    await acpService.setAgents(overrides)
    await acpService.connect('devin')
    await acpService.disconnect('devin')
    await acpService.status()
    await acpService.generateCommitMessage('p1', '/co', 'devin')
    await acpService.cancel('p1', '/co')
    expect(invoke.mock.calls).toEqual([
      ['acp_agents_get', undefined],
      ['acp_agents_set', { overrides }],
      ['acp_connect', { agentId: 'devin' }],
      ['acp_disconnect', { agentId: 'devin' }],
      ['acp_status', undefined],
      ['acp_generate_commit_message', { projectId: 'p1', checkoutPath: '/co', agentId: 'devin' }],
      ['acp_cancel', { projectId: 'p1', checkoutPath: '/co' }],
    ])
  })

  it('turns a rejected invoke into a BridgeError', async () => {
    invoke.mockRejectedValueOnce({ code: 'internal', message: 'cancelled', hint: null })
    await expect(acpService.generateCommitMessage('p', '/c', 'devin')).rejects.toEqual({
      code: 'internal',
      message: 'cancelled',
      hint: null,
    })
  })
})
