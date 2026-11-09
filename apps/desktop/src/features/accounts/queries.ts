import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { queryKeys } from '../../queryClient'
import { accountsService, type AccountAgent, type AccountsView } from '../../services/accounts'
import type { BridgeError } from '../../services/errors'
import type { SessionInfo } from '../../services/types'
import { UNASSIGNED_WORKSPACE, useUi } from '../../stores/ui'
import { useCreateSession } from '../sessions/queries'

export function useAccounts() {
  return useQuery<AccountsView, BridgeError>({
    queryKey: queryKeys.accounts,
    queryFn: () => accountsService.get(),
  })
}

// Every command returns the whole AccountsView, so a success rewrites the
// cache entry; the usage snapshot is stale because the active account (and
// possibly the per-account map) changed.
function useAccountsMutation<TVariables>(mutationFn: (variables: TVariables) => Promise<AccountsView>) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn,
    onSuccess: (view) => {
      queryClient.setQueryData(queryKeys.accounts, view)
      void queryClient.invalidateQueries({ queryKey: queryKeys.usage })
    },
  })
}

export interface AddAccountInput {
  agent: AccountAgent
  name: string
  dir: string
}

export interface RenameAccountInput {
  agent: AccountAgent
  id: string
  name: string
}

export interface AccountTarget {
  agent: AccountAgent
  id: string
}

export function useAddAccount() {
  return useAccountsMutation(({ agent, name, dir }: AddAccountInput) =>
    accountsService.add(agent, name, dir),
  )
}

export function useRenameAccount() {
  return useAccountsMutation(({ agent, id, name }: RenameAccountInput) =>
    accountsService.rename(agent, id, name),
  )
}

export function useRemoveAccount() {
  return useAccountsMutation(({ agent, id }: AccountTarget) => accountsService.remove(agent, id))
}

export function useSetActiveAccount() {
  return useAccountsMutation(({ agent, id }: AccountTarget) => accountsService.setActive(agent, id))
}

/**
 * Runs an added account's own login command (`accounts_sign_in`) in a new
 * terminal session and opens it under Unassigned. A rejection from either
 * step is the mutation error; the account itself stays added.
 */
export function useSignInAccount() {
  const createSession = useCreateSession()
  return useMutation<SessionInfo, BridgeError, AccountTarget>({
    mutationFn: async ({ agent, id }) => {
      const launch = await accountsService.signIn(agent, id)
      const session = await createSession.mutateAsync({
        command: launch.command,
        cwd: launch.cwd,
        title: launch.title,
        cols: 80,
        rows: 24,
      })
      useUi.getState().openSession(UNASSIGNED_WORKSPACE, session.id, session.title || launch.title)
      return session
    },
  })
}
