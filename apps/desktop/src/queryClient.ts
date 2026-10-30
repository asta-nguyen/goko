import { QueryClient } from '@tanstack/react-query'
import type { SessionInfo } from './services/types'

export const queryKeys = {
  sessions: ['sessions'],
  daemonStatus: ['daemon-status'],
} as const

// The daemon pushes changes and the bridge already reconnects, so never refetch on our own.
export const makeQueryClient = () =>
  new QueryClient({
    defaultOptions: { queries: { staleTime: Infinity, refetchOnWindowFocus: false, retry: false } },
  })

export const queryClient = makeQueryClient()

/**
 * Writes one session into `['sessions']` (the daemon is the authority: unknown ids are inserted).
 * Cancels an in-flight fetch first so it cannot overwrite the newer data. With no cached list yet,
 * a lone session would hide the rest, so refetch instead.
 */
export async function upsertSession(qc: QueryClient, session: SessionInfo): Promise<void> {
  await qc.cancelQueries({ queryKey: queryKeys.sessions })
  if (qc.getQueryData(queryKeys.sessions) === undefined) {
    await qc.invalidateQueries({ queryKey: queryKeys.sessions })
    return
  }
  qc.setQueryData<SessionInfo[]>(queryKeys.sessions, (prev = []) =>
    prev.some((s) => s.id === session.id) ? prev.map((s) => (s.id === session.id ? session : s)) : [...prev, session],
  )
}
