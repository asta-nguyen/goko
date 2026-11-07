import { call } from './call'
import type { GitCommitResult, GitPushResult } from './types'

export type GitDiscardLayer = 'unstaged' | 'untracked'

// Git writes stay inside a validated project checkout (the Rust side re-checks
// it on each request); `paths: null` stages or unstages the whole worktree.
export const gitWriteService = {
  stage: (projectId: string, checkoutPath: string, paths: string[] | null) =>
    call<void>('git_write_stage', { projectId, checkoutPath, paths }),
  unstage: (projectId: string, checkoutPath: string, paths: string[] | null) =>
    call<void>('git_write_unstage', { projectId, checkoutPath, paths }),
  discard: (projectId: string, checkoutPath: string, layer: GitDiscardLayer, path: string) =>
    call<void>('git_write_discard', { projectId, checkoutPath, layer, path }),
  commit: (projectId: string, checkoutPath: string, message: string) =>
    call<GitCommitResult>('git_write_commit', { projectId, checkoutPath, message }),
  push: (projectId: string, checkoutPath: string) =>
    call<GitPushResult>('git_write_push', { projectId, checkoutPath }),
}

export type GitWriteService = typeof gitWriteService
