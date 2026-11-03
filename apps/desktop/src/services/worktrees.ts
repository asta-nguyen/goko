import { call } from './call'
import type {
  EnvCopyReport,
  EnvFileCandidate,
  WorktreeBranches,
  WorktreeInfo,
  WorktreeList,
} from './types'

export const worktreeService = {
  list: (projectId: string) => call<WorktreeList>('worktree_list', { projectId }),
  branches: (projectId: string) => call<WorktreeBranches>('worktree_branches', { projectId }),
  create: (input: { projectId: string; path: string; mode: 'new' | 'existing'; branch: string; base: string | null }) =>
    call<WorktreeInfo>('worktree_create', input),
  remove: (projectId: string, path: string) => call<void>('worktree_remove', { projectId, path }),
  envCandidates: (projectId: string, sourcePath: string) =>
    call<EnvFileCandidate[]>('env_file_candidates', { projectId, sourcePath }),
  envCopy: (projectId: string, sourcePath: string, targetPath: string) =>
    call<EnvCopyReport>('env_copy', { projectId, sourcePath, targetPath }),
}

export type WorktreeService = typeof worktreeService
