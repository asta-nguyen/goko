import { Channel, invoke } from '@tauri-apps/api/core'
import { toBridgeError } from './errors'
import type { DaemonStatus, SessionInfo } from './types'

export interface CreateSessionInput {
  command: string
  cwd: string
  cols: number
  rows: number
  title?: string
}

/** Every rejection leaves this module as a `BridgeError`. */
async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args)
  } catch (e) {
    throw toBridgeError(e)
  }
}

export const sessionService = {
  create: (input: CreateSessionInput) => call<SessionInfo>('session_create', { ...input }),
  stop: (id: string, force: boolean) => call<void>('session_stop', { id, force }),
  list: () => call<SessionInfo[]>('session_list'),
  write: (id: string, data: string) => call<void>('terminal_write', { id, data }),
  resize: (id: string, cols: number, rows: number) => call<void>('terminal_resize', { id, cols, rows }),
  status: () => call<DaemonStatus>('daemon_status'),

  /** Output frames are `[u64 BE seq][bytes]`; the returned function unsubscribes (once). */
  async subscribe(id: string, onBytes: (seq: number, data: Uint8Array) => void): Promise<() => void> {
    const onOutput = new Channel<ArrayBuffer>((buf) => {
      if (buf.byteLength < 8) return
      // ponytail: Number() is exact below 2^53 bytes of output per session.
      onBytes(Number(new DataView(buf).getBigUint64(0)), new Uint8Array(buf, 8))
    })
    await call('terminal_subscribe', { id, onOutput })
    let done = false
    return () => {
      if (done) return
      done = true
      // Nothing to do if this fails (e.g. daemon gone): the bridge drops the subscription anyway.
      void call('terminal_unsubscribe', { id }).catch(() => {})
    }
  },
}

export type SessionService = typeof sessionService
