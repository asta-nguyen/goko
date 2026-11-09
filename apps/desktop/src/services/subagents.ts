import { call } from './call'

export interface Subagent {
  agent: string
  pid: number
  command: string
}

export type SubagentMap = Record<string, Subagent[]>

export const subagentsService = {
  get: () => call<SubagentMap>('subagents_get'),
}
