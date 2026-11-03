// Hand-mirrored from crates/protocol (SessionState, SessionInfo) and the s1-09 `daemon_status` command.
export type SessionState =
  | 'created'
  | 'starting'
  | 'running'
  | 'waiting_for_input'
  | 'idle'
  | 'stopping'
  | 'exited'
  | 'failed'
  | 'disconnected'
  | 'interrupted'
  | 'unknown'

export interface SessionInfo {
  id: string
  title: string
  argv: string[]
  cwd: string
  state: SessionState
  exit_code: number | null
  error: string | null
  /** Unix epoch milliseconds. */
  started_at: number
  /** Unix epoch milliseconds. */
  ended_at: number | null
}

export interface DaemonStatus {
  connected: boolean
  live_cwd_supported?: boolean | null
  default_shell: string
  home: string
}

// Hand-mirrored from src-tauri `ProjectView`: the stored row plus `missing` at read time.
export interface Project {
  id: string
  name: string
  root: string
  /** Unix epoch milliseconds. */
  created_at: number
  /** The root is gone from disk. */
  missing: boolean
}

// Hand-mirrored from src-tauri `AgentInfo` (`agent_list`).
export interface AgentInfo {
  id: string
  label: string
  available: boolean
  /** Resolved on PATH; `null` when the agent is not installed. */
  path: string | null
}

// Hand-mirrored from src-tauri `SessionMeta` (`session_meta_list`); app-owned, joins
// daemon `SessionInfo.id` to `session_id`. A daemon session with no row is unassigned.
export interface SessionMeta {
  session_id: string
  project_id: string | null
  /** The adapter id, or `custom` for a free-form command. */
  agent: string
  argv: string[]
  cwd: string
  title: string
  /** Unix epoch milliseconds. */
  created_at: number
}

export interface WorktreeInfo {
  name: string
  path: string
  branch: string | null
  head: string
  is_root: boolean
  missing: boolean
  dirty: boolean | null
  changed: number | null
}

export interface WorktreeList {
  repo: boolean
  worktrees: WorktreeInfo[]
}

export interface BranchInfo {
  name: string
  checked_out_at: string | null
}

export interface BaseRef {
  name: string
  kind: 'local' | 'remote'
}

export interface WorktreeBranches {
  branches: BranchInfo[]
  bases: BaseRef[]
}

export interface EnvFileCandidate {
  name: string
  eligible: boolean
  reason: string | null
}

export interface EnvCopyFailure {
  name: string
  error: string
}

export interface EnvCopyReport {
  copied: string[]
  skipped: string[]
  failed: EnvCopyFailure[]
}

// Hand-mirrored from src-tauri `git_view.rs` (the `git_view_*` read commands).
export type GitViewStatus =
  | 'modified'
  | 'added'
  | 'deleted'
  | 'renamed'
  | 'copied'
  | 'typechange'
  | 'unmerged'
  | 'untracked'

export interface GitViewStatusEntry {
  /** Checkout-relative path; the destination on a rename. */
  path: string
  /** Rename or copy source path, when Git reported one. */
  orig_path: string | null
  status: GitViewStatus
}

export interface GitViewChanges {
  /** Symbolic branch name; `null` on a detached HEAD. */
  branch: string | null
  /** Abbreviated HEAD oid; empty on an unborn branch. */
  head: string
  conflicts: GitViewStatusEntry[]
  staged: GitViewStatusEntry[]
  unstaged: GitViewStatusEntry[]
  untracked: GitViewStatusEntry[]
}

export type GitViewDiffLayer = 'staged' | 'unstaged' | 'untracked' | 'conflict' | 'commit'

export interface GitViewDiff {
  /** Human-readable layer label, e.g. `HEAD → index` or `stash@{2}`. */
  label: string
  patch: string
  /** The unified diff exceeded 1 MiB and was cut. */
  truncated: boolean
  /** Changed files for commit and stash layers; empty for per-file layers. */
  files: GitViewStatusEntry[]
}

export interface GitViewCommit {
  /** Full commit oid. */
  id: string
  parents: string[]
  /** Local branches, `remotes/`-style remote-tracking refs, and `tag:` names. */
  refs: string[]
  /** `true` when this commit is the checkout's HEAD. */
  head: boolean
  author: string
  /** Commit time, epoch milliseconds. */
  time: number
  subject: string
}

export interface GitViewHistory {
  commits: GitViewCommit[]
  /** Skip offset for the next page; `null` at the end. */
  next_skip: number | null
}

export interface GitViewTreeEntry {
  /** Checkout-relative path. */
  path: string
  /** Last path component. */
  name: string
  kind: 'dir' | 'file'
}

export interface GitViewTree {
  /** One directory level: subdirectories first, then files. */
  entries: GitViewTreeEntry[]
}

export type GitViewFileKind = 'text' | 'binary' | 'nonutf8' | 'missing' | 'unreadable'

export interface GitViewFile {
  path: string
  kind: GitViewFileKind
  /** Present only for `text` (possibly truncated) previews. */
  text: string | null
  /** The file exceeded 1 MiB and the preview was cut. */
  truncated: boolean
  /** File size in bytes. */
  size: number
}

export interface GitViewStash {
  /** `stash@{N}` position. */
  index: number
  /** Full stash commit oid — immutable identity for detail requests. */
  id: string
  /** Stash message (`WIP on …`/`On …:` suffix when Git recorded one). */
  message: string
  /** Branch hint parsed from the message, when present. */
  branch: string | null
  /** Stash commit time, epoch milliseconds. */
  time: number
}
