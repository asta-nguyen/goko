import { call } from './call'
import type { AgentInfo } from './types'

export const agentService = {
  list: () => call<AgentInfo[]>('agent_list'),
}

export type AgentService = typeof agentService
