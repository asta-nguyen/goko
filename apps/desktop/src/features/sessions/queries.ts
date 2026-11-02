import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { queryKeys, upsertSession } from '../../queryClient'
import { agentService } from '../../services/agents'
import type { BridgeError } from '../../services/errors'
import { type CreateSessionInput, sessionService } from '../../services/session'
import type { AgentInfo, DaemonStatus, SessionInfo, SessionMeta } from '../../services/types'

export function useSessionList() {
  return useQuery<SessionInfo[], BridgeError>({
    queryKey: queryKeys.sessions,
    queryFn: sessionService.list,
  })
}

/** Selected from the list: an unchanged session keeps its object identity. */
export function useSession(id: string | null) {
  return useQuery<SessionInfo[], BridgeError, SessionInfo | undefined>({
    queryKey: queryKeys.sessions,
    queryFn: sessionService.list,
    select: (list) => list.find((s) => s.id === id),
  })
}

export function useDaemonStatus() {
  return useQuery<DaemonStatus, BridgeError>({
    queryKey: queryKeys.daemonStatus,
    queryFn: sessionService.status,
  })
}

export function useAgentList() {
  return useQuery<AgentInfo[], BridgeError>({
    queryKey: queryKeys.agents,
    queryFn: agentService.list,
  })
}

/** Display name for a session's agent id; `custom` is the free-form command. */
export const agentLabel = (agents: AgentInfo[], id: string) =>
  agents.find((a) => a.id === id)?.label ?? (id === 'custom' ? 'Custom' : id)

export function useSessionMetaList() {
  return useQuery<SessionMeta[], BridgeError>({
    queryKey: queryKeys.sessionMeta,
    queryFn: sessionService.meta,
  })
}

export function useCreateSession() {
  const qc = useQueryClient()
  return useMutation<SessionInfo, BridgeError, CreateSessionInput>({
    mutationFn: sessionService.create,
    onSuccess: async (session) => {
      // The metadata row was written during create. Refresh it first so the new session
      // lands under its project, never flashing under Unassigned.
      await qc.invalidateQueries({ queryKey: queryKeys.sessionMeta })
      await upsertSession(qc, session)
    },
  })
}

/** The state change arrives as an event, not from this call. */
export function useStopSession() {
  return useMutation<void, BridgeError, { id: string; force: boolean }>({
    mutationFn: ({ id, force }) => sessionService.stop(id, force),
  })
}
