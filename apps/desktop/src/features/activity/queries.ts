import { useQuery } from '@tanstack/react-query'
import { queryKeys } from '../../queryClient'
import { activityService, type Activity } from '../../services/activity'
import type { SessionInfo } from '../../services/types'

export function useActivity() {
  return useQuery({
    queryKey: queryKeys.activity,
    queryFn: activityService.get,
  })
}

export function useSessionActivity(session: SessionInfo): Activity | null {
  const { data } = useActivity()
  return session.state === 'running' ? (data?.[session.id] ?? null) : null
}
