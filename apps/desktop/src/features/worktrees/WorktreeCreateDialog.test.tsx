import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient } from '../../queryClient'
import type { Project, WorktreeInfo } from '../../services/types'
import { WorktreeCreateDialog } from './WorktreeCreateDialog'

const { service, chooseDirectory } = vi.hoisted(() => ({
  service: { list: vi.fn(), branches: vi.fn(), create: vi.fn(), remove: vi.fn(), envCandidates: vi.fn(), envCopy: vi.fn() },
  chooseDirectory: vi.fn(),
}))
vi.mock('../../services/worktrees', () => ({ worktreeService: service }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: chooseDirectory }))

const project: Project = { id: 'p1', name: 'sample', root: '/work/sample', created_at: 1, missing: false }
const root: WorktreeInfo = {
  name: 'sample', path: project.root, branch: 'main', head: '1234567890abcdef', is_root: true,
  missing: false, dirty: false, changed: 0,
}
const linked: WorktreeInfo = { ...root, name: 'feature', path: '/work/feature', branch: 'feature', is_root: false }
const branches = {
  branches: [
    { name: 'main', checked_out_at: project.root },
    { name: 'feature', checked_out_at: '/work/feature' },
    { name: 'spare', checked_out_at: null },
  ],
  bases: [{ name: 'main', kind: 'local' as const }, { name: 'origin/main', kind: 'remote' as const }],
}

let qc: QueryClient
const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>
const renderDialog = (onCreated = vi.fn()) => {
  render(
    <WorktreeCreateDialog
      project={project}
      worktrees={[root, linked]}
      selectedCheckout={root}
      open
      onOpenChange={vi.fn()}
      onFocusReturn={vi.fn()}
      onCreated={onCreated}
    />,
    { wrapper },
  )
  return onCreated
}

beforeEach(() => {
  qc = makeQueryClient()
  service.list.mockReset().mockResolvedValue({ repo: true, worktrees: [root] })
  service.branches.mockReset().mockResolvedValue(branches)
  service.create.mockReset().mockResolvedValue({ ...linked, path: '/work/sample-worktrees/feature-2' })
  service.remove.mockReset().mockResolvedValue(undefined)
  service.envCandidates.mockReset().mockResolvedValue([
    { name: '.env', eligible: true, reason: null },
    { name: '.env.local', eligible: false, reason: 'already exists' },
  ])
  service.envCopy.mockReset().mockResolvedValue({ copied: ['.env'], skipped: ['.env.local'], failed: [] })
  chooseDirectory.mockReset().mockResolvedValue(null)
})

describe('WorktreeCreateDialog', () => {
  it('defaults to a new branch beside the project and submits the selected base', async () => {
    const onCreated = renderDialog()
    expect((await screen.findByLabelText('Name') as HTMLInputElement).value).toBe('worktree')
    expect((screen.getByLabelText('Path') as HTMLInputElement).value).toBe('../sample-worktrees/worktree')
    expect((screen.getByLabelText('Branch name') as HTMLInputElement).value).toBe('worktree')
    expect((screen.getByLabelText('Start from') as HTMLSelectElement).value).toBe('main')
    expect((screen.getByRole('checkbox', { name: /Copy environment files into this worktree/ }) as HTMLInputElement).checked).toBe(false)

    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'feature-2' } })
    expect((screen.getByLabelText('Path') as HTMLInputElement).value).toBe('../sample-worktrees/feature-2')
    fireEvent.change(screen.getByLabelText('Path'), { target: { value: '../alternate/renamed' } })
    expect((screen.getByLabelText('Name') as HTMLInputElement).value).toBe('renamed')
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'feature-2' } })
    expect((screen.getByLabelText('Path') as HTMLInputElement).value).toBe('../alternate/feature-2')
    fireEvent.click(screen.getByRole('button', { name: 'Create worktree' }))

    await waitFor(() => expect(service.create).toHaveBeenCalledWith({
      projectId: 'p1', path: '../alternate/feature-2', mode: 'new', branch: 'feature-2', base: 'main',
    }, expect.anything()))
    expect(onCreated).toHaveBeenCalledWith(expect.objectContaining({ path: '/work/sample-worktrees/feature-2' }), null)
  })

  it('offers only free local branches and copies eligible environment files from the chosen checkout', async () => {
    const onCreated = renderDialog()
    await screen.findByLabelText('Name')
    const existingOption = screen.getByRole('radio', { name: 'Existing local branch' })
    fireEvent.click(existingOption)
    const choices = screen.getAllByRole('option') as HTMLOptionElement[]
    expect(choices.find((option) => option.value === 'feature')?.disabled).toBe(true)
    expect(choices.find((option) => option.value === 'feature')?.textContent).toContain('checked out at /work/feature')
    expect(choices.find((option) => option.value === 'spare')?.disabled).toBe(false)
    fireEvent.change(screen.getByLabelText('Local branch'), { target: { value: 'spare' } })

    fireEvent.click(screen.getByRole('checkbox', { name: /Copy environment files into this worktree/ }))
    expect(await screen.findByText('.env.local')).toBeTruthy()
    fireEvent.change(screen.getByLabelText('Copy from'), { target: { value: '/work/feature' } })
    await waitFor(() => expect(service.envCandidates).toHaveBeenCalledWith('p1', '/work/feature'))
    fireEvent.click(screen.getByRole('button', { name: 'Create worktree' }))

    await waitFor(() => expect(service.create).toHaveBeenCalledWith({
      projectId: 'p1', path: '../sample-worktrees/worktree', mode: 'existing', branch: 'spare', base: null,
    }, expect.anything()))
    await waitFor(() => expect(service.envCopy).toHaveBeenCalledWith('p1', '/work/feature', '/work/sample-worktrees/feature-2'))
    expect(onCreated).toHaveBeenCalledWith(
      expect.objectContaining({ path: '/work/sample-worktrees/feature-2' }),
      { sourcePath: '/work/feature', targetPath: '/work/sample-worktrees/feature-2', report: { copied: ['.env'], skipped: ['.env.local'], failed: [] }, error: null },
    )
  })

  it('announces checkout-name collisions inline and keeps the form open', async () => {
    renderDialog()
    await screen.findByLabelText('Name')
    const submit = screen.getByRole('button', { name: 'Create worktree' })
    await waitFor(() => expect(submit).toHaveProperty('disabled', false))
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'feature' } })
    fireEvent.click(submit)

    expect((await screen.findByRole('alert')).textContent).toMatch(/checkout named.*feature.*already exists/i)
    expect(screen.getByRole('dialog', { name: 'New worktree' })).toBeTruthy()
    expect(service.create).not.toHaveBeenCalled()
  })

  it('keeps the new worktree result and reports partial environment-copy failures', async () => {
    service.envCopy.mockResolvedValue({ copied: [], skipped: [], failed: [{ name: '.env', error: 'permission denied' }] })
    const onCreated = renderDialog()
    await screen.findByLabelText('Name')
    fireEvent.click(screen.getByRole('checkbox', { name: /Copy environment files into this worktree/ }))
    fireEvent.click(screen.getByRole('button', { name: 'Create worktree' }))

    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(
      expect.objectContaining({ path: '/work/sample-worktrees/feature-2' }),
      { sourcePath: project.root, targetPath: '/work/sample-worktrees/feature-2', report: { copied: [], skipped: [], failed: [{ name: '.env', error: 'permission denied' }] }, error: null },
    ))
  })

  it('defaults a detached checkout to its current commit', async () => {
    render(
      <WorktreeCreateDialog
        project={project}
        worktrees={[root]}
        selectedCheckout={{ ...root, branch: null, head: 'fedcba9876543210' }}
        open
        onOpenChange={vi.fn()}
        onFocusReturn={vi.fn()}
        onCreated={vi.fn()}
      />,
      { wrapper },
    )

    const baseSelect = await screen.findByLabelText('Start from') as HTMLSelectElement
    await waitFor(() => expect(baseSelect).toHaveProperty('disabled', false))
    expect(baseSelect.value).toBe('fedcba9876543210')
    expect(baseSelect.querySelector('option')?.textContent).toMatch(/detached fedcba9876543210 \(commit\)/)
  })
})
