import { call } from './call'
import type { AcpAgent, AcpOverride, AcpStatusMap } from './types'

// ACP agents are long-lived child processes owned by the backend hub; the
// frontend connects, disconnects, polls status, and asks for a draft message.
export const acpService = {
  agents: () => call<AcpAgent[]>('acp_agents_get'),
  setAgents: (overrides: AcpOverride[]) => call<AcpAgent[]>('acp_agents_set', { overrides }),
  connect: (agentId: string) => call<void>('acp_connect', { agentId }),
  disconnect: (agentId: string) => call<void>('acp_disconnect', { agentId }),
  status: () => call<AcpStatusMap>('acp_status'),
  generateCommitMessage: (projectId: string, checkoutPath: string, agentId: string) =>
    call<string>('acp_generate_commit_message', { projectId, checkoutPath, agentId }),
  cancel: (projectId: string, checkoutPath: string) => call<void>('acp_cancel', { projectId, checkoutPath }),
}

export type AcpService = typeof acpService
