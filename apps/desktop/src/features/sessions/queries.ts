import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { queryKeys, upsertSession } from '../../queryClient'
import type { BridgeError } from '../../services/errors'
import { type CreateSessionInput, sessionService } from '../../services/session'
import type { DaemonStatus, SessionInfo } from '../../services/types'

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

export function useCreateSession() {
  const qc = useQueryClient()
  return useMutation<SessionInfo, BridgeError, CreateSessionInput>({
    mutationFn: sessionService.create,
    onSuccess: (session) => upsertSession(qc, session),
  })
}

/** The state change arrives as an event, not from this call. */
export function useStopSession() {
  return useMutation<void, BridgeError, { id: string; force: boolean }>({
    mutationFn: ({ id, force }) => sessionService.stop(id, force),
  })
}
