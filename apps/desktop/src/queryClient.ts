import { QueryClient } from '@tanstack/react-query'
import type { SessionInfo } from './services/types'

export const queryKeys = {
  sessions: ['sessions'],
  sessionMeta: ['session-meta'],
  projects: ['projects'],
  agents: ['agents'],
  worktrees: (projectId: string) => ['worktrees', projectId] as const,
  worktreeBranches: (projectId: string) => ['worktree-branches', projectId] as const,
  envCandidates: (projectId: string, sourcePath: string) => ['env-candidates', projectId, sourcePath] as const,
  // Git view reads are checkout-scoped: two worktrees of one project never
  // share status, diff, history, tree, or preview entries.
  gitChanges: (projectId: string, checkoutPath: string) => ['git-changes', projectId, checkoutPath] as const,
  gitDiff: (projectId: string, checkoutPath: string, layer: string | null, path: string | null, commit: string | null) =>
    ['git-diff', projectId, checkoutPath, layer, path, commit] as const,
  gitHistory: (projectId: string, checkoutPath: string, branch?: string | null) =>
    branch === undefined
      ? (['git-history', projectId, checkoutPath] as const)
      : (['git-history', projectId, checkoutPath, branch ?? 'all'] as const),
  gitBranches: (projectId: string, checkoutPath: string) => ['git-branches', projectId, checkoutPath] as const,
  gitTree: (projectId: string, checkoutPath: string, directory: string) =>
    ['git-tree', projectId, checkoutPath, directory] as const,
  gitFileList: (projectId: string, checkoutPath: string) => ['git-file-list', projectId, checkoutPath] as const,
  gitFile: (projectId: string, checkoutPath: string, path: string) =>
    ['git-file', projectId, checkoutPath, path] as const,
  gitSearch: (projectId: string, checkoutPath: string, query: string) =>
    ['git-search', projectId, checkoutPath, query] as const,
  // `refs/stash` is repository-wide, so the list key drops the checkout and the
  // detail key uses the stash's immutable commit id.
  gitStashes: (projectId: string) => ['git-stashes', projectId] as const,
  gitStashDiff: (projectId: string, stashId: string) => ['git-stash-diff', projectId, stashId] as const,
  lastAgent: ['settings', 'last-agent'],
  editorVim: ['settings', 'editor.vim'] as const,
  daemonStatus: ['daemon-status'],
  usage: ['usage'],
  activity: ['activity'] as const,
} as const

// The daemon pushes changes and the bridge already reconnects, so never refetch on our own.
export const makeQueryClient = () =>
  new QueryClient({
    defaultOptions: { queries: { staleTime: Infinity, refetchOnWindowFocus: false, retry: false } },
  })

export const queryClient = makeQueryClient()

/**
 * Writes one session into `['sessions']` (the daemon is the authority: unknown ids are inserted).
 * Cancels an in-flight fetch first so it cannot overwrite the newer data. With no cached list yet,
 * a lone session would hide the rest, so refetch instead.
 */
export async function upsertSession(qc: QueryClient, session: SessionInfo): Promise<void> {
  await qc.cancelQueries({ queryKey: queryKeys.sessions })
  if (qc.getQueryData(queryKeys.sessions) === undefined) {
    await qc.invalidateQueries({ queryKey: queryKeys.sessions })
    return
  }
  qc.setQueryData<SessionInfo[]>(queryKeys.sessions, (prev = []) =>
    prev.some((s) => s.id === session.id) ? prev.map((s) => (s.id === session.id ? session : s)) : [...prev, session],
  )
}
