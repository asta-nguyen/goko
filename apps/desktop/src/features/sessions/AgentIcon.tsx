import { SquareTerminalIcon, TerminalIcon } from 'lucide-react'
import { cn } from '@/lib/utils'
import { AGENT_LOGO_PATHS } from './agent-logos'

/** Marks a session or launch-menu entry with its agent; shell and custom commands use a terminal glyph. */
export function AgentIcon({ agent, className }: { agent?: string; className?: string }) {
  const paths = agent ? AGENT_LOGO_PATHS[agent] : undefined
  if (paths)
    return (
      <svg
        viewBox="0 0 24 24"
        fill={agent === 'claude' ? '#D97757' : 'currentColor'}
        fillRule="evenodd"
        clipRule="evenodd"
        aria-hidden
        className={cn('size-4 shrink-0 text-text', className)}
      >
        {paths.map((d) => (
          <path key={d} d={d} />
        ))}
      </svg>
    )
  const Icon = agent === 'shell' ? SquareTerminalIcon : TerminalIcon
  return <Icon aria-hidden className={cn('size-4 shrink-0 text-muted', className)} />
}
