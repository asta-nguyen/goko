import { invoke } from '@tauri-apps/api/core'
import { toBridgeError } from './errors'

/** Every rejection leaves this module as a `BridgeError`. */
export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args)
  } catch (e) {
    throw toBridgeError(e)
  }
}
