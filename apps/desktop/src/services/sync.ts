import type { QueryClient } from '@tanstack/react-query'
import { listen } from '@tauri-apps/api/event'
import { queryKeys, upsertSession } from '../queryClient'
import type { UsageSnapshot } from './usage'
import type { DaemonStatus, SessionInfo } from './types'

const active = new WeakMap<QueryClient, () => void>()

/** Writes daemon events into the query cache. Idempotent per client; returns the stop function. */
export function startQuerySync(qc: QueryClient): () => void {
  const running = active.get(qc)
  if (running) return running

  // The bridge forwards the daemon's full `SessionInfo` for both session events.
  const onSession = ({ payload }: { payload: SessionInfo }) => void upsertSession(qc, payload)
  const unlisteners = [
    listen<SessionInfo>('session://updated', onSession),
    listen<SessionInfo>('session://exited', onSession),
    listen<{ connected: boolean }>('daemon://status', ({ payload }) => {
      if (payload.connected) {
        void qc.invalidateQueries({ queryKey: queryKeys.sessions })
        void qc.invalidateQueries({ queryKey: queryKeys.daemonStatus })
      } else {
        qc.setQueryData<DaemonStatus>(queryKeys.daemonStatus, (s) => s && { ...s, connected: false })
      }
    }),
    listen<UsageSnapshot>('usage://updated', ({ payload }) => {
      qc.setQueryData(queryKeys.usage, payload)
    }),
  ]

  // The bridge may have connected before these listeners existed, and that `connected` event
  // is gone for good, so ask again once they are in place.
  let stopped = false
  void Promise.all(unlisteners).then(() => {
    if (stopped) return
    void qc.invalidateQueries({ queryKey: queryKeys.daemonStatus })
    void qc.invalidateQueries({ queryKey: queryKeys.sessions })
  })

  const stop = () => {
    stopped = true
    active.delete(qc)
    // `listen` is async: unlisten whenever it resolves, even if we stop first.
    for (const u of unlisteners) void u.then((unlisten) => unlisten())
  }
  active.set(qc, stop)
  return stop
}
