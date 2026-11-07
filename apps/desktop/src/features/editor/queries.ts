import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { queryKeys } from '../../queryClient'
import { acpService } from '../../services/acp'
import type { BridgeError } from '../../services/errors'
import { gitViewService, type GitDiffRequest } from '../../services/git-view'
import { gitWriteService, type GitDiscardLayer } from '../../services/git-write'
import { settingsService } from '../../services/settings'
import type {
  AcpAgent,
  AcpOverride,
  AcpStatusMap,
  GitViewBranch,
  GitViewChanges,
  GitViewDiff,
  GitViewFile,
  GitViewHistory,
  GitViewSearch,
  GitViewStash,
  GitViewTree,
} from '../../services/types'

/** Changes status/diff and the stash list poll at this rate while their section is visible. */
export const GIT_VIEW_POLL_MS = 3000

/** ACP connection status polls at this rate while its consumer is visible. */
export const ACP_STATUS_POLL_MS = 3000

// Every git-view query overrides the global infinite staleTime: entering a
// section fetches fresh data while the last successful result stays visible
// (and a failed refresh keeps it behind a stale indicator). Polling pauses
// while the window is in the background; regaining focus refetches at once.
const READ = { staleTime: 0, refetchOnWindowFocus: true, refetchIntervalInBackground: false } as const

export function useEditorVim() {
  const queryClient = useQueryClient()
  const setting = useQuery<string | null, BridgeError>({
    queryKey: queryKeys.editorVim,
    queryFn: () => settingsService.get('editor.vim'),
    staleTime: Infinity,
  })
  const write = useMutation({
    mutationFn: (enabled: boolean) => settingsService.set('editor.vim', enabled ? '1' : '0'),
    onSuccess: (_result, enabled) => queryClient.setQueryData(queryKeys.editorVim, enabled ? '1' : '0'),
  })
  return [setting.data === '1' && !setting.isError, write.mutate] as const
}

export function useGitChanges(projectId: string, checkoutPath: string, enabled: boolean, poll = enabled) {
  return useQuery<GitViewChanges, BridgeError>({
    queryKey: queryKeys.gitChanges(projectId, checkoutPath),
    queryFn: () => gitViewService.changes(projectId, checkoutPath),
    enabled,
    refetchInterval: enabled && poll ? GIT_VIEW_POLL_MS : false,
    ...READ,
  })
}

export function useGitDiff(
  projectId: string,
  checkoutPath: string,
  request: GitDiffRequest | null,
  options: { enabled?: boolean; poll?: boolean } = {},
) {
  const enabled = (options.enabled ?? true) && request !== null
  return useQuery<GitViewDiff, BridgeError>({
    queryKey: queryKeys.gitDiff(
      projectId,
      checkoutPath,
      request?.layer ?? null,
      request?.path ?? null,
      request?.commit ?? null,
    ),
    queryFn: () => gitViewService.diff(projectId, checkoutPath, request as GitDiffRequest),
    enabled,
    refetchInterval: enabled && options.poll ? GIT_VIEW_POLL_MS : false,
    ...READ,
  })
}

export function useGitHistory(projectId: string, checkoutPath: string, branch: string | null, enabled: boolean) {
  return useInfiniteQuery<GitViewHistory, BridgeError>({
    queryKey: queryKeys.gitHistory(projectId, checkoutPath, branch),
    queryFn: ({ pageParam }) => gitViewService.history(projectId, checkoutPath, pageParam as number, branch),
    initialPageParam: 0,
    getNextPageParam: (last) => last.next_skip ?? undefined,
    enabled,
    ...READ,
  })
}

export function useGitBranches(projectId: string, checkoutPath: string, enabled: boolean) {
  return useQuery<GitViewBranch[], BridgeError>({
    queryKey: queryKeys.gitBranches(projectId, checkoutPath),
    queryFn: () => gitViewService.branches(projectId, checkoutPath),
    enabled,
    ...READ,
  })
}

export function useGitFileList(projectId: string, checkoutPath: string, enabled: boolean) {
  return useQuery<string[], BridgeError>({
    queryKey: queryKeys.gitFileList(projectId, checkoutPath),
    queryFn: () => gitViewService.fileList(projectId, checkoutPath),
    enabled,
    ...READ,
  })
}

export function useGitSearch(projectId: string, checkoutPath: string, query: string, enabled: boolean) {
  return useQuery<GitViewSearch, BridgeError>({
    queryKey: queryKeys.gitSearch(projectId, checkoutPath, query),
    queryFn: () => gitViewService.search(projectId, checkoutPath, query),
    enabled,
    placeholderData: (previous) => previous,
    gcTime: 30_000,
    ...READ,
  })
}

export function useGitTree(projectId: string, checkoutPath: string, directory: string, enabled: boolean) {
  return useQuery<GitViewTree, BridgeError>({
    queryKey: queryKeys.gitTree(projectId, checkoutPath, directory),
    queryFn: () => gitViewService.tree(projectId, checkoutPath, directory),
    enabled,
    ...READ,
  })
}

export function useGitFile(projectId: string, checkoutPath: string, path: string | null, enabled: boolean) {
  return useQuery<GitViewFile, BridgeError>({
    queryKey: queryKeys.gitFile(projectId, checkoutPath, path ?? ''),
    queryFn: () => gitViewService.file(projectId, checkoutPath, path as string),
    enabled: enabled && path !== null,
    ...READ,
  })
}

// `refs/stash` is shared by every linked worktree of the project, so the list
// is keyed by project alone; `checkoutPath` only picks which checkout runs the
// read. It polls at the same cadence as Changes while the section is visible.
export function useGitStashes(projectId: string, checkoutPath: string, enabled: boolean) {
  return useQuery<GitViewStash[], BridgeError>({
    queryKey: queryKeys.gitStashes(projectId),
    queryFn: () => gitViewService.stashes(projectId, checkoutPath),
    enabled,
    refetchInterval: enabled ? GIT_VIEW_POLL_MS : false,
    ...READ,
  })
}

// The detail key is the stash's immutable commit id: the same stash returns
// the same patch from any checkout of the repository.
export function useGitStashDiff(projectId: string, checkoutPath: string, stashId: string | null, enabled: boolean) {
  return useQuery<GitViewDiff, BridgeError>({
    queryKey: queryKeys.gitStashDiff(projectId, stashId ?? ''),
    queryFn: () => gitViewService.stashDiff(projectId, checkoutPath, stashId as string),
    enabled: enabled && stashId !== null,
    ...READ,
  })
}

// A successful write changed the index or worktree, so the changes list and
// every open diff for that checkout are stale; a failure changed nothing.
function useGitWriteInvalidation(projectId: string, checkoutPath: string) {
  const queryClient = useQueryClient()
  return {
    onSuccess: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.gitChanges(projectId, checkoutPath) }),
        queryClient.invalidateQueries({ queryKey: ['git-diff', projectId, checkoutPath] }),
      ]),
  }
}

export function useStage(projectId: string, checkoutPath: string) {
  return useMutation({
    mutationFn: ({ paths }: { paths: string[] | null }) => gitWriteService.stage(projectId, checkoutPath, paths),
    ...useGitWriteInvalidation(projectId, checkoutPath),
  })
}

export function useUnstage(projectId: string, checkoutPath: string) {
  return useMutation({
    mutationFn: ({ paths }: { paths: string[] | null }) =>
      gitWriteService.unstage(projectId, checkoutPath, paths),
    ...useGitWriteInvalidation(projectId, checkoutPath),
  })
}

export function useDiscard(projectId: string, checkoutPath: string) {
  return useMutation({
    mutationFn: ({ layer, path }: { layer: GitDiscardLayer; path: string }) =>
      gitWriteService.discard(projectId, checkoutPath, layer, path),
    ...useGitWriteInvalidation(projectId, checkoutPath),
  })
}

export function useCommit(projectId: string, checkoutPath: string) {
  return useMutation({
    mutationFn: (message: string) => gitWriteService.commit(projectId, checkoutPath, message),
    ...useGitWriteInvalidation(projectId, checkoutPath),
  })
}

export function usePush(projectId: string, checkoutPath: string) {
  return useMutation({
    mutationFn: () => gitWriteService.push(projectId, checkoutPath),
    ...useGitWriteInvalidation(projectId, checkoutPath),
  })
}

export function useAcpAgents() {
  return useQuery<AcpAgent[], BridgeError>({
    queryKey: queryKeys.acpAgents,
    queryFn: () => acpService.agents(),
    staleTime: Infinity,
  })
}

export function useSetAcpAgents() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (overrides: AcpOverride[]) => acpService.setAgents(overrides),
    onSuccess: (agents) => {
      queryClient.setQueryData(queryKeys.acpAgents, agents)
      void queryClient.invalidateQueries({ queryKey: queryKeys.acpStatus })
    },
  })
}

export function useAcpStatus(enabled: boolean) {
  return useQuery<AcpStatusMap, BridgeError>({
    queryKey: queryKeys.acpStatus,
    queryFn: () => acpService.status(),
    enabled,
    refetchInterval: enabled ? ACP_STATUS_POLL_MS : false,
    staleTime: 0,
    refetchIntervalInBackground: false,
  })
}

// Status refetches on settle, not just success: a failed connect still lands
// the agent in `error`, and disconnect always returns it to `disconnected`.
export function useAcpConnect() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (agentId: string) => acpService.connect(agentId),
    onSettled: () => queryClient.invalidateQueries({ queryKey: queryKeys.acpStatus }),
  })
}

export function useAcpDisconnect() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (agentId: string) => acpService.disconnect(agentId),
    onSettled: () => queryClient.invalidateQueries({ queryKey: queryKeys.acpStatus }),
  })
}

export function useGenerateMessage(projectId: string, checkoutPath: string) {
  const queryClient = useQueryClient()
  const generate = useMutation({
    mutationFn: (agentId: string) => acpService.generateCommitMessage(projectId, checkoutPath, agentId),
    onSettled: () => queryClient.invalidateQueries({ queryKey: queryKeys.acpStatus }),
  })
  return { ...generate, cancel: () => acpService.cancel(projectId, checkoutPath) }
}

export function useCommitAgent() {
  const queryClient = useQueryClient()
  const setting = useQuery<string | null, BridgeError>({
    queryKey: queryKeys.commitAgent,
    queryFn: () => settingsService.get('commit.agent'),
    staleTime: Infinity,
  })
  const write = useMutation({
    mutationFn: (agentId: string) => settingsService.set('commit.agent', agentId),
    onSuccess: (_result, agentId) => queryClient.setQueryData(queryKeys.commitAgent, agentId),
  })
  return [setting.data ?? null, write.mutate] as const
}
