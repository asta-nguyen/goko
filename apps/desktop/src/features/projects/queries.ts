import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { queryKeys } from '../../queryClient'
import type { BridgeError } from '../../services/errors'
import { projectService } from '../../services/projects'
import type { Project } from '../../services/types'

export function useProjectList() {
  return useQuery<Project[], BridgeError>({
    queryKey: queryKeys.projects,
    queryFn: projectService.list,
  })
}

export function useAddProject() {
  const qc = useQueryClient()
  return useMutation<Project, BridgeError, string>({
    mutationFn: projectService.add,
    onSuccess: () => qc.invalidateQueries({ queryKey: queryKeys.projects }),
  })
}

/** Metadata rows lose their project association (`ON DELETE SET NULL`), so refresh both. */
export function useRemoveProject() {
  const qc = useQueryClient()
  return useMutation<void, BridgeError, string>({
    mutationFn: projectService.remove,
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: queryKeys.projects })
      void qc.invalidateQueries({ queryKey: queryKeys.sessionMeta })
    },
  })
}
