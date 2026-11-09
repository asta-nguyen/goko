import { useQuery } from '@tanstack/react-query'
import { queryKeys } from '../../queryClient'
import { subagentsService, type Subagent } from '../../services/subagents'
import type { SessionInfo } from '../../services/types'

export function useSubagents() {
  return useQuery({
    queryKey: queryKeys.subagents,
    queryFn: subagentsService.get,
  })
}

const NO_SUBAGENTS: Subagent[] = []

export function useSessionSubagents(session: SessionInfo): Subagent[] {
  const { data } = useSubagents()
  return session.state === 'running' ? (data?.[session.id] ?? NO_SUBAGENTS) : NO_SUBAGENTS
}
