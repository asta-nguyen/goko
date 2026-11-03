import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { queryKeys } from '../../queryClient'
import type { BridgeError } from '../../services/errors'
import { worktreeService } from '../../services/worktrees'
import type {
  EnvCopyReport,
  EnvFileCandidate,
  WorktreeBranches,
  WorktreeInfo,
  WorktreeList,
} from '../../services/types'

export function useWorktreeList(projectId: string, enabled = true) {
  return useQuery<WorktreeList, BridgeError>({
    queryKey: queryKeys.worktrees(projectId),
    queryFn: () => worktreeService.list(projectId),
    enabled,
  })
}

export function useWorktreeBranches(projectId: string, enabled = true) {
  return useQuery<WorktreeBranches, BridgeError>({
    queryKey: queryKeys.worktreeBranches(projectId),
    queryFn: () => worktreeService.branches(projectId),
    enabled,
  })
}

export function useEnvFileCandidates(projectId: string, sourcePath: string, enabled = true) {
  return useQuery<EnvFileCandidate[], BridgeError>({
    queryKey: queryKeys.envCandidates(projectId, sourcePath),
    queryFn: () => worktreeService.envCandidates(projectId, sourcePath),
    enabled: enabled && sourcePath.length > 0,
  })
}

async function invalidateWorktrees(qc: ReturnType<typeof useQueryClient>, projectId: string, branches = false) {
  await Promise.all([
    qc.invalidateQueries({ queryKey: queryKeys.worktrees(projectId) }),
    ...(branches ? [qc.invalidateQueries({ queryKey: queryKeys.worktreeBranches(projectId) })] : []),
  ])
}

export function useCreateWorktree() {
  const qc = useQueryClient()
  return useMutation<WorktreeInfo, BridgeError, Parameters<typeof worktreeService.create>[0]>({
    mutationFn: worktreeService.create,
    onSuccess: (_worktree, input) => invalidateWorktrees(qc, input.projectId, true),
  })
}

export function useRemoveWorktree() {
  const qc = useQueryClient()
  return useMutation<void, BridgeError, { projectId: string; path: string }>({
    mutationFn: ({ projectId, path }) => worktreeService.remove(projectId, path),
    onSuccess: (_result, input) => invalidateWorktrees(qc, input.projectId, true),
  })
}

export function useEnvCopy() {
  const qc = useQueryClient()
  return useMutation<EnvCopyReport, BridgeError, { projectId: string; sourcePath: string; targetPath: string }>({
    mutationFn: ({ projectId, sourcePath, targetPath }) => worktreeService.envCopy(projectId, sourcePath, targetPath),
    onSuccess: (_report, input) => invalidateWorktrees(qc, input.projectId),
  })
}
