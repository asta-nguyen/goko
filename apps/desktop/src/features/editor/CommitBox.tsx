import { useEffect, useRef, useState } from 'react'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { cn } from '@/lib/utils'
import type { AcpState, AcpStatus, GitViewChanges } from '../../services/types'
import { errorMessage } from './panes'
import {
  useAcpAgents,
  useAcpConnect,
  useAcpStatus,
  useCommit,
  useCommitAgent,
  useGenerateMessage,
  usePush,
} from './queries'

const DISCARDED_NOTICE = 'The message was edited while generating — the result was discarded.'
const FALLBACK_STATUS: AcpStatus = { state: 'disconnected', message: null }

const STATE_DOT: Record<AcpState, string> = {
  disconnected: 'bg-muted',
  connecting: 'bg-warning',
  connected: 'bg-success',
  error: 'bg-danger',
}

function StatusDot({ state }: { state: AcpState }) {
  return <span aria-hidden className={cn('inline-block size-1.5 shrink-0 rounded-full', STATE_DOT[state])} />
}

/** The commit box under the changed-files list: message draft, agent pick, and the Git writes. */
export function CommitBox({
  projectId,
  checkoutPath,
  changes,
  active,
}: {
  projectId: string
  checkoutPath: string
  changes: GitViewChanges | undefined
  active: boolean
}) {
  const [message, setMessage] = useState('')
  const [alert, setAlert] = useState<string | null>(null)
  const [status, setStatus] = useState<string | null>(null)
  // Bumped on every user edit so a late Generate reply can tell the draft changed.
  const edits = useRef(0)

  const agents = useAcpAgents()
  const acpStatus = useAcpStatus(active)
  const connect = useAcpConnect()
  const generate = useGenerateMessage(projectId, checkoutPath)
  const commit = useCommit(projectId, checkoutPath)
  const push = usePush(projectId, checkoutPath)
  const [storedAgent, setCommitAgent] = useCommitAgent()

  const statusOf = (agentId: string): AcpStatus => acpStatus.data?.[agentId] ?? FALLBACK_STATUS
  const enabledAgents = (agents.data ?? []).filter((agent) => agent.enabled)
  // The stored pick wins while its agent stays enabled; otherwise the first
  // connected agent, then the first enabled one found on PATH.
  const pickedId =
    storedAgent !== null && enabledAgents.some((agent) => agent.id === storedAgent)
      ? storedAgent
      : (enabledAgents.find((agent) => statusOf(agent.id).state === 'connected')?.id ??
        enabledAgents.find((agent) => agent.available)?.id ??
        null)
  const agent = enabledAgents.find((item) => item.id === pickedId) ?? null
  const agentStatus = agent === null ? null : statusOf(agent.id)

  const staged = changes?.staged.length ?? 0
  const conflicts = changes?.conflicts.length ?? 0
  const ahead = changes?.ahead
  const canGenerate =
    agent !== null && agentStatus?.state === 'connected' && staged > 0 && conflicts === 0 && !generate.isPending
  const canCommit = staged > 0 && conflicts === 0 && message.trim() !== '' && !commit.isPending
  const canPush = !push.isPending && (ahead === null || (typeof ahead === 'number' && ahead > 0))
  const pushLabel = push.isPending
    ? 'Push'
    : ahead === null
      ? 'Push new branch'
      : typeof ahead === 'number' && ahead > 0
        ? `Push ↑${ahead}`
        : 'Push'

  const startGenerate = async () => {
    if (!canGenerate || agent === null) return
    const seen = edits.current
    setAlert(null)
    setStatus(null)
    try {
      const reply = await generate.mutateAsync(agent.id)
      if (edits.current === seen) setMessage(reply)
      else setStatus(DISCARDED_NOTICE)
    } catch (error) {
      // A user cancel aborts the backend job; its 'cancelled' rejection is expected.
      if (errorMessage(error) !== 'cancelled') setAlert(errorMessage(error))
    }
  }

  const commitMessage = async () => {
    if (!canCommit) return
    setAlert(null)
    setStatus(null)
    try {
      const result = await commit.mutateAsync(message.trim())
      setMessage('')
      setStatus(`Committed ${result.head} on ${result.branch ?? 'detached HEAD'}`)
    } catch (error) {
      setAlert(errorMessage(error))
    }
  }

  const pushNow = async () => {
    if (!canPush) return
    setAlert(null)
    setStatus(null)
    try {
      const result = await push.mutateAsync()
      setStatus(result.summary)
    } catch (error) {
      setAlert(errorMessage(error))
    }
  }

  const cancelGenerate = () => {
    void generate.cancel().catch(() => {})
  }

  const generateRef = useRef(generate)
  generateRef.current = generate
  // Leaving the checkout (the parent keys this box by it) cancels a running job.
  useEffect(
    () => () => {
      if (generateRef.current.isPending) void generateRef.current.cancel().catch(() => {})
    },
    [],
  )

  const onMessageChange = (value: string) => {
    edits.current += 1
    setMessage(value)
    setStatus(null)
  }

  const onMessageKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
      event.preventDefault()
      void commitMessage()
    }
  }

  const showConnect =
    agent !== null &&
    agentStatus !== null &&
    (agentStatus.state === 'disconnected' || agentStatus.state === 'connecting' || agentStatus.state === 'error')

  return (
    <div className="flex shrink-0 flex-col gap-2 border-t border-border p-2">
      <textarea
        aria-label="Commit message"
        placeholder="Commit message"
        value={message}
        onChange={(event) => onMessageChange(event.target.value)}
        onKeyDown={onMessageKeyDown}
        rows={3}
        spellCheck={false}
        className="w-full resize-y rounded-md border border-input bg-bg px-2.5 py-1.5 text-sm transition-colors outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
      />
      {alert !== null && (
        <Alert variant="destructive">
          <AlertDescription>{alert}</AlertDescription>
        </Alert>
      )}
      <div className="flex items-center gap-1.5">
        <Select value={pickedId ?? ''} onValueChange={(id) => setCommitAgent(id)}>
          <SelectTrigger aria-label="Commit message agent" size="sm" className="min-w-0 flex-1">
            <SelectValue placeholder="No ACP agent" />
          </SelectTrigger>
          <SelectContent>
            {enabledAgents.map((item) => {
              const itemStatus = statusOf(item.id)
              return (
                <SelectItem
                  key={item.id}
                  value={item.id}
                  disabled={!item.available}
                  title={item.available ? undefined : `${item.label} was not found on PATH`}
                >
                  <span className="flex min-w-0 flex-1 items-center gap-2">
                    <StatusDot state={itemStatus.state} />
                    <span className="truncate">{item.label}</span>
                    <span className="ml-auto shrink-0 pl-3 text-xs text-muted">{itemStatus.state}</span>
                  </span>
                </SelectItem>
              )
            })}
          </SelectContent>
        </Select>
        {showConnect && agent !== null && agentStatus !== null && (
          <Button
            variant="outline"
            size="sm"
            disabled={connect.isPending || agentStatus.state === 'connecting'}
            title={agentStatus.state === 'error' ? (agentStatus.message ?? undefined) : undefined}
            onClick={() => connect.mutate(agent.id)}
          >
            {connect.isPending || agentStatus.state === 'connecting' ? 'Connecting…' : 'Connect'}
          </Button>
        )}
      </div>
      {agent === null && <p className="m-0 text-xs text-muted">Set up an ACP agent in Settings</p>}
      <div className="flex items-center gap-1.5">
        {generate.isPending ? (
          <>
            <Button variant="outline" size="sm" onClick={cancelGenerate}>
              Cancel
            </Button>
            <span role="status" className="min-w-0 flex-1 truncate text-xs text-muted">
              Asking {agent?.label ?? 'agent'}…
            </span>
          </>
        ) : (
          <Button variant="outline" size="sm" disabled={!canGenerate} onClick={() => void startGenerate()}>
            Generate
          </Button>
        )}
        <Button size="sm" disabled={!canCommit} onClick={() => void commitMessage()}>
          Commit
        </Button>
        <Button variant="outline" size="sm" disabled={!canPush} onClick={() => void pushNow()}>
          {pushLabel}
        </Button>
      </div>
      {status !== null && (
        <p role="status" className="m-0 text-xs text-muted">
          {status}
        </p>
      )}
    </div>
  )
}
