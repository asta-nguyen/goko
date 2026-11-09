import { useState, type FormEvent } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { DropdownMenuItem, DropdownMenuSeparator } from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { TooltipProvider } from '@/components/ui/tooltip'
import { useAccounts, useAddAccount, useRemoveAccount, useRenameAccount, useSetActiveAccount, useSignInAccount } from '../accounts/queries'
import { AccountUsageSummary } from '../accounts/AccountUsageSummary'
import { useUsage } from '../usage/queries'
import { RowActionsMenu } from '../sessions/SessionLaunch'
import { isBridgeError } from '../../services/errors'
import type { AccountAgent, AccountView, AccountsView } from '../../services/accounts'
import type { AccountUsage } from '../../services/usage'

const errorMessage = (error: unknown) =>
  isBridgeError(error) ? error.message : error instanceof Error ? error.message : String(error)

const HINT =
  'Settings, skills, and plugins are linked from the Default folder — credentials are never shared, and removing an account keeps its folder.'

const CARDS = [
  {
    agent: 'claude',
    title: 'Claude Code',
    folderPrefix: '~/.claude-',
    description: 'Several Claude Code logins side by side; new sessions use the active account.',
  },
  {
    agent: 'codex',
    title: 'Codex',
    folderPrefix: '~/.codex-',
    description: 'Several Codex logins side by side; new sessions use the active account.',
  },
] as const

function AccountRow({
  agent,
  account,
  isActive,
  usage,
  signInPending,
  onSignIn,
}: {
  agent: AccountAgent
  account: AccountView
  isActive: boolean
  usage: AccountUsage | undefined
  signInPending: boolean
  onSignIn: () => void
}) {
  const setActive = useSetActiveAccount()
  const rename = useRenameAccount()
  const remove = useRemoveAccount()
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(account.name)

  const save = (event: FormEvent) => {
    event.preventDefault()
    const name = draft.trim()
    if (name === '') return
    rename.mutate({ agent, id: account.id, name }, { onSuccess: () => setEditing(false) })
  }

  return (
    <div role="group" aria-label={account.name} className="flex flex-col gap-1.5 rounded-md border border-border p-3">
      <div className="flex items-center gap-2">
        {editing ? (
          <form onSubmit={save} className="flex min-w-0 flex-1 items-center gap-2">
            <Input
              aria-label={`Rename ${account.name}`}
              autoFocus
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              className="h-7 w-56"
            />
            <Button type="submit" size="xs" disabled={rename.isPending || draft.trim() === ''}>
              Save
            </Button>
            <Button type="button" variant="ghost" size="xs" onClick={() => setEditing(false)}>
              Cancel
            </Button>
          </form>
        ) : (
          <>
            <span className="min-w-0 truncate text-sm font-medium">{account.name}</span>
            {isActive && <Badge variant="secondary">Active</Badge>}
            <span className="flex-1" />
            {!isActive && (
              <Button
                variant="outline"
                size="sm"
                disabled={setActive.isPending}
                onClick={() => setActive.mutate({ agent, id: account.id })}
              >
                Set active
              </Button>
            )}
            {!account.is_default && (
              <Button variant="outline" size="sm" disabled={signInPending} onClick={onSignIn}>
                Sign in
              </Button>
            )}
            {!account.is_default && (
              <RowActionsMenu label={`More actions for ${account.name}`}>
                <DropdownMenuItem
                  onSelect={() => {
                    rename.reset()
                    setDraft(account.name)
                    setEditing(true)
                  }}
                >
                  Rename
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  variant="destructive"
                  disabled={remove.isPending}
                  onSelect={() => remove.mutate({ agent, id: account.id })}
                >
                  Remove
                </DropdownMenuItem>
              </RowActionsMenu>
            )}
          </>
        )}
      </div>
      <div className="truncate font-mono text-xs text-muted">{account.dir}</div>
      <AccountUsageSummary agent={agent} usage={usage} />
      {editing && rename.isError && rename.error && (
        <p role="alert" className="m-0 text-xs text-danger">
          {errorMessage(rename.error)}
        </p>
      )}
    </div>
  )
}

function AddAccountForm({
  agent,
  folderPrefix,
  onClose,
  onAdded,
}: {
  agent: AccountAgent
  folderPrefix: string
  onClose: () => void
  onAdded: (view: AccountsView) => void
}) {
  const add = useAddAccount()
  const [name, setName] = useState('')
  const [folder, setFolder] = useState<string | null>(null)
  const slug = name.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-')
  const folderValue = folder ?? `${folderPrefix}${slug}`
  const canSubmit = name.trim() !== '' && folderValue.trim() !== ''

  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (!canSubmit) return
    add.mutate(
      { agent, name: name.trim(), dir: folderValue.trim() },
      { onSuccess: onAdded },
    )
  }

  return (
    <form
      aria-label="Add account"
      onSubmit={submit}
      className="flex flex-col gap-2 rounded-md border border-border p-3"
    >
      <div className="flex items-center gap-2">
        <Input
          aria-label="Name"
          autoFocus
          placeholder="Name"
          value={name}
          onChange={(event) => setName(event.target.value)}
          className="h-8 w-52"
        />
        <Input
          aria-label="Folder"
          placeholder="Folder"
          value={folderValue}
          onChange={(event) => setFolder(event.target.value)}
          spellCheck={false}
          className="h-8 min-w-0 flex-1 font-mono"
        />
      </div>
      {add.isError && add.error && (
        <p role="alert" className="m-0 text-xs text-danger">
          {errorMessage(add.error)}
        </p>
      )}
      <span className="flex gap-1.5">
        <Button type="submit" size="sm" disabled={add.isPending || !canSubmit}>
          Add account
        </Button>
        <Button type="button" variant="ghost" size="sm" onClick={onClose}>
          Cancel
        </Button>
      </span>
    </form>
  )
}

function AgentAccountsCard({
  agent,
  title,
  description,
  folderPrefix,
}: {
  agent: AccountAgent
  title: string
  description: string
  folderPrefix: string
}) {
  const accountsQuery = useAccounts()
  const usageQuery = useUsage()
  const signIn = useSignInAccount()
  const [adding, setAdding] = useState(false)
  const agentView = accountsQuery.data?.[agent]
  const usageAccounts = usageQuery.data?.accounts

  // The add made the new account active, so its id is the view's active id.
  // Sign-in starts here, not in a form callback: the form unmounts on close.
  const handleAdded = (view: AccountsView) => {
    setAdding(false)
    signIn.mutate({ agent, id: view[agent].active })
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent>
        <div role="group" aria-label={`${title} accounts`} className="flex flex-col gap-2">
          {accountsQuery.isPending ? (
            <p className="m-0 text-sm text-muted">Loading accounts…</p>
          ) : accountsQuery.isError ? (
            <p role="alert" className="m-0 text-sm text-danger">
              {errorMessage(accountsQuery.error)}
            </p>
          ) : (
            agentView?.items.map((account) => (
              <AccountRow
                key={account.id}
                agent={agent}
                account={account}
                isActive={agentView.active === account.id}
                usage={usageAccounts?.[`${agent}:${account.id}`]}
                signInPending={signIn.isPending}
                onSignIn={() => signIn.mutate({ agent, id: account.id })}
              />
            ))
          )}
          {adding ? (
            <AddAccountForm
              agent={agent}
              folderPrefix={folderPrefix}
              onClose={() => setAdding(false)}
              onAdded={handleAdded}
            />
          ) : (
            <div>
              <Button variant="outline" size="sm" onClick={() => setAdding(true)}>
                Add account
              </Button>
            </div>
          )}
          {signIn.isError && signIn.error && (
            <p role="alert" className="m-0 text-xs text-danger">
              {errorMessage(signIn.error)}
            </p>
          )}
          <p className="m-0 text-xs text-muted">{HINT}</p>
        </div>
      </CardContent>
    </Card>
  )
}

export function AccountsSection() {
  return (
    <TooltipProvider>
      <div className="flex flex-col gap-4">
        {CARDS.map((card) => (
          <AgentAccountsCard key={card.agent} {...card} />
        ))}
      </div>
    </TooltipProvider>
  )
}
