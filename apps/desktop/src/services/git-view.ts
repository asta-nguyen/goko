import { call } from './call'
import type {
  GitViewChanges,
  GitViewDiff,
  GitViewDiffLayer,
  GitViewFile,
  GitViewHistory,
  GitViewStash,
  GitViewTree,
} from './types'

/** One diff pane request: a file layer, or a whole-commit patch. */
export interface GitDiffRequest {
  layer: GitViewDiffLayer
  path?: string
  commit?: string
}

// Read-only Git inspection for the workspace Editor; every call stays inside a
// validated project checkout (the Rust side re-checks it on each request).
export const gitViewService = {
  changes: (projectId: string, checkoutPath: string) =>
    call<GitViewChanges>('git_view_changes', { projectId, checkoutPath }),
  diff: (projectId: string, checkoutPath: string, request: GitDiffRequest) =>
    call<GitViewDiff>('git_view_diff', {
      projectId,
      checkoutPath,
      layer: request.layer,
      path: request.path ?? null,
      commit: request.commit ?? null,
    }),
  history: (projectId: string, checkoutPath: string, skip: number) =>
    call<GitViewHistory>('git_view_history', { projectId, checkoutPath, skip }),
  tree: (projectId: string, checkoutPath: string, directory: string) =>
    call<GitViewTree>('git_view_tree', { projectId, checkoutPath, directory }),
  file: (projectId: string, checkoutPath: string, path: string) =>
    call<GitViewFile>('git_view_file', { projectId, checkoutPath, path }),
  stashes: (projectId: string, checkoutPath: string) =>
    call<GitViewStash[]>('git_view_stashes', { projectId, checkoutPath }),
  stashDiff: (projectId: string, checkoutPath: string, stashId: string) =>
    call<GitViewDiff>('git_view_stash_diff', { projectId, checkoutPath, stashId }),
}

export type GitViewService = typeof gitViewService
