import { open as chooseDirectory } from '@tauri-apps/plugin-dialog'
import { type FormEvent, useEffect, useState } from 'react'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { isBridgeError } from '../../services/errors'
import type { EnvCopyReport, WorktreeInfo, WorktreeList, Project } from '../../services/types'
import { useCreateWorktree, useEnvCopy, useEnvFileCandidates, useWorktreeBranches } from './queries'

export interface WorktreeEnvResult {
  sourcePath: string
  targetPath: string
  report: EnvCopyReport | null
  error: string | null
}

const message = (error: unknown) => isBridgeError(error) ? error.message : error instanceof Error ? error.message : String(error)
const dirname = (path: string) => {
  const value = path.replace(/\/+$/, '')
  const separator = value.lastIndexOf('/')
  return separator < 0 ? '' : value.slice(0, separator)
}
const basename = (path: string) => path.replace(/\/+$/, '').split('/').at(-1) ?? ''
const withName = (path: string, name: string) => {
  const parent = dirname(path)
  return parent ? `${parent}/${name}` : path.startsWith('/') ? `/${name}` : name
}
const selectClass = 'h-9 w-full rounded-md border border-input bg-bg px-2.5 text-sm text-text outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:opacity-50'

export function WorktreeCreateDialog({
  project,
  worktrees,
  selectedCheckout,
  open,
  onOpenChange,
  onFocusReturn,
  onCreated,
}: {
  project: Project
  worktrees: WorktreeList['worktrees']
  selectedCheckout: WorktreeInfo
  open: boolean
  onOpenChange: (open: boolean) => void
  onFocusReturn: () => void
  onCreated: (worktree: WorktreeInfo, envResult: WorktreeEnvResult | null) => void
}) {
  const branches = useWorktreeBranches(project.id, open)
  const candidates = useCreateWorktree()
  const envCopy = useEnvCopy()
  const [name, setName] = useState('worktree')
  const [path, setPath] = useState(`../${project.name}-worktrees/worktree`)
  const [mode, setMode] = useState<'new' | 'existing'>('new')
  const [branch, setBranch] = useState('worktree')
  const [branchTouched, setBranchTouched] = useState(false)
  const [base, setBase] = useState(selectedCheckout.branch ?? selectedCheckout.head)
  const [copyEnvironment, setCopyEnvironment] = useState(false)
  const [sourcePath, setSourcePath] = useState(project.root)
  const [formError, setFormError] = useState<string | null>(null)

  useEffect(() => {
    if (!open) return
    setName('worktree')
    setPath(`../${project.name}-worktrees/worktree`)
    setMode('new')
    setBranch('worktree')
    setBranchTouched(false)
    setBase(selectedCheckout.branch ?? selectedCheckout.head)
    setCopyEnvironment(false)
    setSourcePath(project.root)
    setFormError(null)
  }, [open, project.id, project.name, project.root, selectedCheckout.branch, selectedCheckout.head])

  const envFiles = useEnvFileCandidates(project.id, sourcePath, open && copyEnvironment)
  const freeBranches = branches.data?.branches.filter((item) => item.checked_out_at === null) ?? []
  const disabled = candidates.isPending || envCopy.isPending

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    setFormError(null)
    if (!name.trim() || name !== name.trim() || name === '.' || name === '..' || /[\\/]/.test(name)) {
      setFormError('Name must be one directory name.')
      return
    }
    if (worktrees.some((item) => item.name === name)) {
      setFormError(`A checkout named “${name}” already exists.`)
      return
    }
    if (!path.trim()) {
      setFormError('Choose a path for the worktree.')
      return
    }
    if (mode === 'new') {
      if (!branch.trim()) {
        setFormError('Enter a branch name.')
        return
      }
      if (!branches.data || branches.error) {
        setFormError('Could not load local branches. Retry before creating a worktree.')
        return
      }
      if (branches.data.branches.some((item) => item.name === branch)) {
        setFormError(`Branch “${branch}” already exists. Choose Existing branch or use another name.`)
        return
      }
    } else {
      const selected = branches.data?.branches.find((item) => item.name === branch)
      if (!branches.data || branches.error) {
        setFormError('Could not load local branches. Retry before creating a worktree.')
        return
      }
      if (!selected || selected.checked_out_at) {
        setFormError(selected?.checked_out_at
          ? `Branch “${branch}” is checked out at ${selected.checked_out_at}.`
          : 'Choose an existing local branch.')
        return
      }
    }

    try {
      const created = await candidates.mutateAsync({
        projectId: project.id,
        path: path.trim(),
        mode,
        branch: branch.trim(),
        base: mode === 'new' ? base : null,
      })
      let envResult: WorktreeEnvResult | null = null
      if (copyEnvironment) {
        try {
          const report = await envCopy.mutateAsync({ projectId: project.id, sourcePath, targetPath: created.path })
          envResult = { sourcePath, targetPath: created.path, report, error: null }
        } catch (error) {
          envResult = { sourcePath, targetPath: created.path, report: null, error: message(error) }
        }
      }
      onCreated(created, envResult)
      onOpenChange(false)
    } catch (error) {
      setFormError(message(error))
    }
  }

  const choosePath = async () => {
    setFormError(null)
    try {
      const selected = await chooseDirectory({ directory: true, defaultPath: project.root })
      if (typeof selected !== 'string') return
      setPath(selected)
      const nextName = basename(selected)
      setName(nextName)
      if (mode === 'new' && !branchTouched) setBranch(nextName)
    } catch (error) {
      setFormError(message(error))
    }
  }

  const currentBase = selectedCheckout.branch ?? selectedCheckout.head
  const hasCurrentBase = branches.data?.bases.some((item) => item.name === currentBase) ?? false

  return (
    <Dialog open={open} onOpenChange={(next) => { if (!disabled) onOpenChange(next) }}>
      <DialogContent aria-describedby="worktree-create-description" onCloseAutoFocus={(event) => { event.preventDefault(); onFocusReturn() }}>
        <DialogHeader>
          <DialogTitle>New worktree</DialogTitle>
          <DialogDescription id="worktree-create-description">
            Create an independent checkout inside {project.name}. It stays part of this project.
          </DialogDescription>
        </DialogHeader>
        <form className="flex flex-col gap-4" onSubmit={(event) => void submit(event)}>
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="worktree-name">Name</Label>
              <Input
                id="worktree-name"
                autoFocus
                value={name}
                aria-invalid={Boolean(formError && !name.trim())}
                onChange={(event) => {
                  const nextName = event.currentTarget.value
                  setName(nextName)
                  setPath((current) => withName(current, nextName))
                  if (mode === 'new' && !branchTouched) setBranch(nextName)
                }}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="worktree-path">Path</Label>
              <div className="flex gap-2">
                <Input
                  id="worktree-path"
                  value={path}
                  onChange={(event) => {
                    const nextPath = event.currentTarget.value
                    setPath(nextPath)
                    const nextName = basename(nextPath)
                    if (nextName) {
                      setName(nextName)
                      if (mode === 'new' && !branchTouched) setBranch(nextName)
                    }
                  }}
                />
                <Button type="button" variant="outline" disabled={disabled} onClick={() => void choosePath()}>Choose…</Button>
              </div>
            </div>
          </div>

          <fieldset className="flex flex-col gap-3 rounded-lg border border-border p-3">
            <legend className="px-1 text-sm font-medium">Branch</legend>
            <div className="flex flex-wrap gap-x-5 gap-y-2 text-sm">
              <label className="flex items-center gap-2">
                <input type="radio" name="branch-mode" value="new" checked={mode === 'new'} onChange={() => { setMode('new'); setBranch(name); setBranchTouched(false) }} />
                New branch
              </label>
              <label className="flex items-center gap-2">
                <input type="radio" name="branch-mode" value="existing" checked={mode === 'existing'} disabled={freeBranches.length === 0} onChange={() => { setMode('existing'); setBranch(freeBranches[0]?.name ?? '') }} />
                Existing local branch
              </label>
            </div>
            {mode === 'new' ? (
              <div className="grid gap-3 sm:grid-cols-2">
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="worktree-branch">Branch name</Label>
                  <Input id="worktree-branch" value={branch} onChange={(event) => { setBranch(event.currentTarget.value); setBranchTouched(true) }} />
                </div>
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="worktree-base">Start from</Label>
                  <select id="worktree-base" className={selectClass} value={base} onChange={(event) => setBase(event.currentTarget.value)} disabled={branches.isLoading}>
                    {!hasCurrentBase && <option value={currentBase}>{selectedCheckout.branch ?? `detached ${selectedCheckout.head} (commit)`}</option>}
                    {branches.data?.bases.map((item) => <option key={`${item.kind}:${item.name}`} value={item.name}>{item.name}{item.kind === 'remote' ? ' (remote)' : ''}</option>)}
                  </select>
                </div>
              </div>
            ) : (
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="existing-branch">Local branch</Label>
                <select id="existing-branch" className={selectClass} value={branch} onChange={(event) => setBranch(event.currentTarget.value)}>
                  {branches.data?.branches.map((item) => (
                    <option key={item.name} value={item.name} disabled={item.checked_out_at !== null}>
                      {item.name}{item.checked_out_at ? ` — checked out at ${item.checked_out_at}` : ''}
                    </option>
                  ))}
                  {branches.data?.branches.length === 0 && <option value="">No local branches</option>}
                </select>
              </div>
            )}
            <p className="m-0 text-xs text-muted">Goko uses refs already available locally and does not fetch.</p>
          </fieldset>

          <fieldset className="flex flex-col gap-3 rounded-lg border border-border p-3">
            <legend className="px-1 text-sm font-medium">Environment files</legend>
            <label className="flex items-start gap-2 text-sm">
              <input type="checkbox" checked={copyEnvironment} onChange={(event) => setCopyEnvironment(event.currentTarget.checked)} />
              <span>Copy environment files into this worktree <span className="block text-xs text-muted">These files may contain secrets. They are copied locally and are not committed.</span></span>
            </label>
            {copyEnvironment && (
              <>
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="env-source">Copy from</Label>
                  <select id="env-source" className={selectClass} value={sourcePath} onChange={(event) => setSourcePath(event.currentTarget.value)}>
                    {worktrees.map((item) => (
                      <option key={item.path} value={item.path} disabled={item.missing}>
                        {item.name}{item.is_root ? ' (project root)' : ''}{item.missing ? ' — missing' : ''}
                      </option>
                    ))}
                  </select>
                </div>
                <div className="rounded-md bg-bg/70 p-2.5">
                  <p className="mb-1 text-xs font-medium">Copy preview</p>
                  {envFiles.isLoading ? <p className="m-0 text-xs text-muted">Scanning files…</p> : null}
                  {envFiles.error && <p role="alert" className="m-0 text-xs text-danger">{envFiles.error.message}</p>}
                  {envFiles.data?.length === 0 && <p className="m-0 text-xs text-muted">No environment files found.</p>}
                  {envFiles.data?.length ? (
                    <ul className="m-0 flex list-none flex-col gap-1 p-0 text-xs">
                      {envFiles.data.map((item) => <li key={item.name} className="flex justify-between gap-3"><span className="truncate">{item.name}</span><span className={item.eligible ? 'text-success' : 'text-muted'}>{item.eligible ? 'copy' : item.reason ?? 'skip'}</span></li>)}
                    </ul>
                  ) : null}
                </div>
              </>
            )}
          </fieldset>

          {formError && <Alert variant="destructive" role="alert"><AlertDescription>{formError}</AlertDescription></Alert>}
          {branches.error && <p role="alert" className="m-0 text-sm text-danger">{branches.error.message}</p>}
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" disabled={disabled} onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button type="submit" disabled={disabled || branches.isLoading || !path.trim()}>{candidates.isPending ? 'Creating…' : 'Create worktree'}</Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  )
}
