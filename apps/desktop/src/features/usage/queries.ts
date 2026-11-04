import { useQuery } from '@tanstack/react-query'
import { queryKeys } from '../../queryClient'
import { claudeSessionId, usageService } from '../../services/usage'
import type { SessionInfo } from '../../services/types'

export function useUsage() {
  return useQuery({
    queryKey: queryKeys.usage,
    queryFn: usageService.get,
  })
}

export function useSessionContext(session: SessionInfo): number | null {
  const { data } = useUsage()
  const id = claudeSessionId(session.argv)
  return id === null ? null : (data?.claude.sessions[id]?.context_pct ?? null)
}
