import { call } from './call'
import type {
  GitViewBranch,
  GitViewChanges,
  GitViewDiff,
  GitViewDiffLayer,
  GitViewFile,
  GitViewHistory,
  GitViewSearch,
  GitViewStash,
  GitViewTree,
} from './types'

/** One diff pane request: a file layer, or a whole-commit patch. */
export interface GitDiffRequest {
  layer: GitViewDiffLayer
  path?: string
  commit?: string
}

export interface GitFileSaveRequest {
  projectId: string
  checkoutPath: string
  path: string
  expectedText: string
  text: string
}

// Git inspection and text-file saving stay inside a validated project checkout
// (the Rust side re-checks it on each request).
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
  history: (projectId: string, checkoutPath: string, skip: number, branch: string | null = null) =>
    call<GitViewHistory>('git_view_history', { projectId, checkoutPath, skip, branch }),
  branches: (projectId: string, checkoutPath: string) =>
    call<GitViewBranch[]>('git_view_branches', { projectId, checkoutPath }),
  tree: (projectId: string, checkoutPath: string, directory: string) =>
    call<GitViewTree>('git_view_tree', { projectId, checkoutPath, directory }),
  fileList: (projectId: string, checkoutPath: string) =>
    call<string[]>('git_view_file_list', { projectId, checkoutPath }),
  search: (projectId: string, checkoutPath: string, query: string) =>
    call<GitViewSearch>('git_view_search', { projectId, checkoutPath, query }),
  file: (projectId: string, checkoutPath: string, path: string) =>
    call<GitViewFile>('git_view_file', { projectId, checkoutPath, path }),
  saveFile: (request: GitFileSaveRequest) => call<GitViewFile>('git_view_file_save', { ...request }),
  stashes: (projectId: string, checkoutPath: string) =>
    call<GitViewStash[]>('git_view_stashes', { projectId, checkoutPath }),
  stashDiff: (projectId: string, checkoutPath: string, stashId: string) =>
    call<GitViewDiff>('git_view_stash_diff', { projectId, checkoutPath, stashId }),
}

export type GitViewService = typeof gitViewService
