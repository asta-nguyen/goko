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
