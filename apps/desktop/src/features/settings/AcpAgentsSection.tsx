import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { isBridgeError } from '../../services/errors'
import type { AcpAgent, AcpOverride, AcpState, AcpStatus } from '../../services/types'
import { useAcpAgents, useAcpConnect, useAcpDisconnect, useAcpStatus, useSetAcpAgents } from '../editor/queries'

const STATUS_DOT: Record<AcpState, string> = {
  disconnected: 'bg-muted',
  connecting: 'bg-warning',
  connected: 'bg-success',
  error: 'bg-danger',
}

const TEXTAREA_CLASS =
  'w-full min-w-0 rounded-md border border-input bg-bg px-2.5 py-1.5 text-sm transition-colors outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50'

const errorMessage = (error: unknown) =>
  isBridgeError(error) ? error.message : error instanceof Error ? error.message : String(error)

// `acp.agents` persists only rows that differ from their preset, so every save
// rewrites the whole override list: untouched overrides ride along with their
// stored values and the edited agent's row is replaced — or dropped on reset.
function overridesFor(agents: AcpAgent[], targetId: string, row: AcpOverride | null): AcpOverride[] {
  return agents.flatMap((agent) => {
    if (agent.id === targetId) return row ? [row] : []
    return agent.is_default
      ? []
      : [{ id: agent.id, enabled: agent.enabled, command: agent.command, prompt: agent.prompt }]
  })
}

function AcpAgentRow({
  agent,
  agents,
  status,
}: {
  agent: AcpAgent
  agents: AcpAgent[]
  status: AcpStatus | undefined
}) {
  const state = status?.state ?? 'disconnected'
  const connect = useAcpConnect()
  const disconnect = useAcpDisconnect()
  const setAgents = useSetAcpAgents()

  // The command and prompt fields are drafts: they keep showing what the user
  // typed until the server value changes — a successful save re-syncs them, a
  // rejected one leaves them alone.
  const [drafts, setDrafts] = useState({ command: agent.command, prompt: agent.prompt })
  const [synced, setSynced] = useState({ command: agent.command, prompt: agent.prompt })
  if (agent.command !== synced.command || agent.prompt !== synced.prompt) {
    setSynced({ command: agent.command, prompt: agent.prompt })
    setDrafts({ command: agent.command, prompt: agent.prompt })
  }

  const persist = (row: AcpOverride | null) => setAgents.mutate(overridesFor(agents, agent.id, row))
  const persistDrafts = (enabled = agent.enabled) =>
    persist({ id: agent.id, enabled, command: drafts.command, prompt: drafts.prompt })

  const unavailable = !agent.available
  const actionDisabled = state === 'connecting' || !agent.enabled || unavailable
  const actionTitle = unavailable ? 'Not found on PATH' : undefined

  return (
    <div role="group" aria-label={agent.label} className="flex flex-col gap-2 rounded-md border border-border p-3">
      <div className="flex items-center gap-2">
        <span className="text-sm font-medium">{agent.label}</span>
        <span className="flex items-center gap-1.5 text-xs text-muted">
          <span
            title={status?.message ?? state}
            aria-hidden
            className={`size-2 shrink-0 rounded-full ${STATUS_DOT[state]}`}
          />
          {state}
        </span>
        {state === 'error' && status?.message && (
          <span className="min-w-0 truncate text-xs text-danger">{status.message}</span>
        )}
        <span className="flex-1" />
        {state === 'connected' ? (
          <Button
            variant="outline"
            size="sm"
            disabled={actionDisabled}
            title={actionTitle}
            onClick={() => disconnect.mutate(agent.id)}
          >
            Disconnect
          </Button>
        ) : state === 'connecting' ? (
          <Button variant="outline" size="sm" disabled title={actionTitle}>
            Connecting…
          </Button>
        ) : (
          <Button
            variant="outline"
            size="sm"
            disabled={actionDisabled}
            title={actionTitle}
            onClick={() => connect.mutate(agent.id)}
          >
            Connect
          </Button>
        )}
        <Switch
          aria-label={`${agent.label} enabled`}
          checked={agent.enabled}
          onCheckedChange={(checked) => persistDrafts(checked)}
        />
      </div>
      <div className="flex flex-col gap-1">
        <span className="text-xs text-muted">Command</span>
        <Input
          aria-label={`${agent.label} command`}
          className="font-mono"
          value={drafts.command}
          onChange={(event) => setDrafts((current) => ({ ...current, command: event.target.value }))}
          onBlur={() => {
            if (drafts.command !== agent.command) persistDrafts()
          }}
        />
      </div>
      <div className="flex flex-col gap-1">
        <span className="text-xs text-muted">Commit prompt</span>
        <textarea
          aria-label={`${agent.label} commit prompt`}
          placeholder="Default prompt"
          rows={2}
          className={TEXTAREA_CLASS}
          value={drafts.prompt}
          onChange={(event) => setDrafts((current) => ({ ...current, prompt: event.target.value }))}
          onBlur={() => {
            if (drafts.prompt !== agent.prompt) persistDrafts()
          }}
        />
      </div>
      {(!agent.is_default || setAgents.isError) && (
        <div className="flex items-start justify-between gap-2">
          {!agent.is_default && (
            <Button variant="ghost" size="xs" aria-label={`Reset ${agent.label}`} onClick={() => persist(null)}>
              Reset to default
            </Button>
          )}
          {setAgents.isError && setAgents.error && (
            <p role="alert" className="m-0 min-w-0 text-xs text-danger">
              {errorMessage(setAgents.error)}
            </p>
          )}
        </div>
      )}
    </div>
  )
}

export function AcpAgentsSection() {
  const agentsQuery = useAcpAgents()
  const agents = agentsQuery.data ?? []
  // This component mounts only while its section is the active one, so `true`
  // polls the status exactly while the section is visible.
  const statusQuery = useAcpStatus(true)

  return (
    <Card>
      <CardHeader>
        <CardTitle>ACP agents</CardTitle>
        <CardDescription>
          Local agents that can draft commit messages from the staged diff. Connect an agent to use it in Changes.
        </CardDescription>
      </CardHeader>
      <CardContent>
        {agentsQuery.isPending ? (
          <p className="m-0 text-sm text-muted">Loading agents…</p>
        ) : agentsQuery.isError ? (
          <p className="m-0 text-sm text-danger">{errorMessage(agentsQuery.error)}</p>
        ) : (
          <div className="flex flex-col gap-3">
            {agents.map((agent) => (
              <AcpAgentRow key={agent.id} agent={agent} agents={agents} status={statusQuery.data?.[agent.id]} />
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  )
}
