import { listen } from '@tauri-apps/api/event'

const SHELL_SPECIAL = /([ \t!"#$&'()*;<>?[\\\]^`{|}])/g

/** Backslash-escapes shell metacharacters, as Terminal.app does for a dropped file. */
export const shellPath = (path: string) => path.replace(SHELL_SPECIAL, '\\$1')

/**
 * Tauri swallows file drops on the window and reports their paths instead. Calls `onDrop` with
 * the escaped paths joined by spaces (plus a trailing space), ready to paste. Returns the stop function.
 */
export function startFileDrop(onDrop: (text: string) => void): () => void {
  const unlisten = listen<{ paths: string[] }>('tauri://drag-drop', ({ payload }) => {
    if (payload.paths.length) onDrop(`${payload.paths.map(shellPath).join(' ')} `)
  })
  return () => void unlisten.then((stop) => stop())
}
