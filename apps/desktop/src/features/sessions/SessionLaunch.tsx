import { useState, type FormEvent } from 'react'
import { MoreHorizontalIcon, PlusIcon } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import type { AgentInfo } from '../../services/types'
import { useAccounts } from '../accounts/queries'
import { AgentIcon } from './AgentIcon'

export type SessionTarget = { agent: string; title?: string } | { command: string; title?: string }

export function NewSessionMenu({
  label,
  agents,
  disabled,
  onAgent,
  onCustom,
}: {
  label: string
  agents: AgentInfo[]
  disabled: boolean
  onAgent: (id: string) => void
  onCustom: () => void
}) {
  const { data: accounts } = useAccounts()
  return (
    <DropdownMenu>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon-sm" aria-label={label} disabled={disabled} className="text-muted hover:text-text">
              <PlusIcon />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent>{label}</TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end" className="w-56">
        {agents.map((agent) => {
          const agentKey = agent.id === 'claude' || agent.id === 'codex' ? agent.id : null
          const agentAccounts = agentKey === null ? undefined : accounts?.[agentKey]
          const activeName =
            agentAccounts !== undefined && agentAccounts.items.length >= 2
              ? agentAccounts.items.find((item) => item.id === agentAccounts.active)?.name
              : undefined
          return (
            <DropdownMenuItem key={agent.id} disabled={disabled || !agent.available} onSelect={() => onAgent(agent.id)}>
              <AgentIcon agent={agent.id} />
              {agent.label}
              {activeName && (
                <>
                  {' '}
                  <span className="text-muted">· {activeName}</span>
                </>
              )}
              {!agent.available && ' — not found on PATH'}
            </DropdownMenuItem>
          )
        })}
        <DropdownMenuSeparator />
        <DropdownMenuItem disabled={disabled} onSelect={onCustom}>
          <AgentIcon />
          Custom command…</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

export function RowActionsMenu({
  label,
  children,
  ...props
}: { label: string; children: React.ReactNode } & React.ComponentProps<typeof Button>) {
  return (
    <DropdownMenu>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon-sm" aria-label={label} className="text-muted hover:text-text" {...props}>
              <MoreHorizontalIcon />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent>{label}</TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end" className="w-56">{children}</DropdownMenuContent>
    </DropdownMenu>
  )
}

export function CustomCommandForm({
  initial,
  disabled,
  onStart,
  onCancel,
}: {
  initial: string
  disabled: boolean
  onStart: (command: string) => void
  onCancel: () => void
}) {
  const [typed, setTyped] = useState<string | null>(null)
  const command = typed ?? initial
  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (command.trim()) onStart(command)
  }
  return (
    <form onSubmit={submit} className="ml-4 flex flex-col gap-1.5 py-1 pl-2">
      <Input
        aria-label="Command"
        autoFocus
        value={command}
        onChange={(event) => setTyped(event.target.value)}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        className="font-mono"
      />
      <span className="flex gap-1.5">
        <Button type="submit" size="sm" disabled={disabled || !command.trim()}>Start</Button>
        <Button type="button" variant="ghost" size="sm" onClick={onCancel}>Cancel</Button>
      </span>
    </form>
  )
}
