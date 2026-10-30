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
