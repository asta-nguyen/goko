import { BookOpenIcon, TerminalSquareIcon } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { WorkspaceView } from '../../stores/ui'

const item = (pressed: boolean) =>
  cn(
    'flex h-7 items-center gap-1.5 rounded px-2 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring',
    pressed ? 'bg-panel font-medium text-text shadow-sm' : 'text-muted hover:text-text',
    'disabled:cursor-not-allowed disabled:opacity-50 disabled:hover:text-muted',
  )

/** The workspace-level Terminal | Editor mode switch, separate from session tabs. */
export function ViewSwitch({
  view,
  editorEnabled = true,
  onChange,
}: {
  view: WorkspaceView
  editorEnabled?: boolean
  onChange: (view: WorkspaceView) => void
}) {
  return (
    <div
      role="group"
      aria-label="Workspace view"
      className="flex shrink-0 items-center gap-0.5 rounded-md border border-border bg-bg p-0.5"
    >
      <button
        type="button"
        aria-pressed={view === 'terminal'}
        onClick={() => onChange('terminal')}
        className={item(view === 'terminal')}
      >
        <TerminalSquareIcon className="size-3.5" aria-hidden />
        Terminal
      </button>
      <button
        type="button"
        aria-pressed={view === 'editor'}
        disabled={!editorEnabled}
        title={editorEnabled ? undefined : 'Editor is available for Git checkouts'}
        onClick={() => onChange('editor')}
        className={item(view === 'editor')}
      >
        <BookOpenIcon className="size-3.5" aria-hidden />
        Editor
      </button>
    </div>
  )
}
