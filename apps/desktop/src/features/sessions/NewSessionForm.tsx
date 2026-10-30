import { open } from '@tauri-apps/plugin-dialog'
import { type FormEvent, useRef, useState } from 'react'
import { useUi } from '../../stores/ui'
import { useCreateSession, useDaemonStatus } from './queries'

const inputClass =
  'w-full rounded border border-border bg-bg px-2 py-1 text-sm text-text disabled:opacity-50'

export function NewSessionForm() {
  const select = useUi((s) => s.select)
  const { data: status } = useDaemonStatus()
  const create = useCreateSession()
  const [cwd, setCwd] = useState<string | null>(null)
  const [typed, setTyped] = useState<string | null>(null) // null = untouched, show the default shell
  const [dialogError, setDialogError] = useState<string | null>(null)
  const inFlight = useRef(false) // `isPending` lags a render, so a fast double-click needs this

  const command = typed ?? status?.default_shell ?? ''
  const ready = cwd !== null && command.trim() !== ''

  async function pickDirectory() {
    setDialogError(null)
    try {
      const dir = await open({ directory: true })
      if (typeof dir === 'string') setCwd(dir) // cancelled → null → unchanged
    } catch (e) {
      setDialogError(e instanceof Error ? e.message : String(e))
    }
  }

  function start(e: FormEvent) {
    e.preventDefault()
    if (cwd === null || !ready || inFlight.current) return
    inFlight.current = true
    create.mutate(
      { command, cwd, cols: 80, rows: 24 }, // the terminal resizes itself on mount
      {
        onSuccess: (session) => select(session.id),
        onSettled: () => (inFlight.current = false),
      },
    )
  }

  return (
    <form onSubmit={start} className="flex flex-col gap-2">
      <fieldset disabled={create.isPending} className="m-0 flex min-w-0 flex-col gap-2 border-0 p-0">
        <button
          type="button"
          onClick={pickDirectory}
          className="rounded border border-border px-2 py-1 text-left text-sm hover:bg-bg disabled:opacity-50"
        >
          Choose directory…
        </button>
        <p className="m-0 truncate text-xs text-muted" title={cwd ?? undefined}>
          {cwd ?? 'No directory chosen'}
        </p>
        <label className="flex flex-col gap-1 text-xs text-muted">
          Command
          <input
            value={command}
            onChange={(e) => setTyped(e.target.value)}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            className={`${inputClass} font-mono`}
          />
        </label>
        <button
          type="submit"
          disabled={!ready}
          className="rounded bg-accent px-2 py-1 text-sm font-medium text-bg disabled:opacity-50"
        >
          Start
        </button>
      </fieldset>
      {!ready && <p className="m-0 text-xs text-muted">Choose a directory and enter a command to start.</p>}
      {dialogError && (
        <p role="alert" className="m-0 text-xs text-danger">
          {dialogError}
        </p>
      )}
      {create.error && (
        <p role="alert" className="m-0 text-xs text-danger">
          {create.error.message}
          {create.error.hint && <span className="block text-muted">{create.error.hint}</span>}
        </p>
      )}
    </form>
  )
}
