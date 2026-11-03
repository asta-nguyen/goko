import { open } from '@tauri-apps/plugin-dialog'
import { useRef, useState } from 'react'
import { isBridgeError } from '../../services/errors'
import { useAddProject } from './queries'

const message = (error: unknown) =>
  isBridgeError(error) ? error.message : error instanceof Error ? error.message : String(error)

/** Pick a folder with the native dialog, then register it as a project. */
export function useAddProjectFolder() {
  const add = useAddProject()
  const [error, setError] = useState<string | null>(null)
  const inFlight = useRef(false) // `isPending` lags a render, so a fast double-click needs this

  async function addProject() {
    if (inFlight.current) return
    inFlight.current = true
    setError(null)
    try {
      const dir = await open({ directory: true })
      if (typeof dir === 'string') await add.mutateAsync(dir) // cancelled: nothing changes
    } catch (caught) {
      setError(message(caught))
    } finally {
      inFlight.current = false
    }
  }

  return { addProject, error, pending: add.isPending }
}
