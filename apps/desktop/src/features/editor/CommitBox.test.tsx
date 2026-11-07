import { QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient } from '../../queryClient'
import type { AcpAgent, GitViewChanges, GitViewStatusEntry } from '../../services/types'
import { CommitBox } from './CommitBox'

const gitWrite = vi.hoisted(() => ({
  stage: vi.fn(),
  unstage: vi.fn(),
  discard: vi.fn(),
  commit: vi.fn(),
  push: vi.fn(),
}))
const acp = vi.hoisted(() => ({
  agents: vi.fn(),
  setAgents: vi.fn(),
  connect: vi.fn(),
  disconnect: vi.fn(),
  status: vi.fn(),
  generateCommitMessage: vi.fn(),
  cancel: vi.fn(),
}))
const settings = vi.hoisted(() => ({ get: vi.fn(), set: vi.fn() }))
const gitView = vi.hoisted(() => ({
  changes: vi.fn(),
  diff: vi.fn(),
  history: vi.fn(),
  tree: vi.fn(),
  fileList: vi.fn(),
  file: vi.fn(),
  search: vi.fn(),
  stashes: vi.fn(),
  stashDiff: vi.fn(),
}))
vi.mock('../../services/git-write', () => ({ gitWriteService: gitWrite }))
vi.mock('../../services/acp', () => ({ acpService: acp }))
vi.mock('../../services/settings', () => ({ settingsService: settings }))
vi.mock('../../services/git-view', () => ({ gitViewService: gitView }))

const devin: AcpAgent = {
  id: 'devin',
  label: 'Devin',
  enabled: true,
  command: 'devin acp',
  prompt: '',
  available: true,
  is_default: true,
}
const claude: AcpAgent = { ...devin, id: 'claude', label: 'Claude', is_default: false }

const stagedEntry: GitViewStatusEntry = { path: 'src/app.ts', orig_path: null, status: 'modified' }

const changes = (over: Partial<GitViewChanges> = {}): GitViewChanges => ({
  branch: 'main',
  head: 'abc1234',
  conflicts: [],
  staged: [stagedEntry],
  unstaged: [],
  untracked: [],
  ahead: 0,
  ...over,
})

let qc: ReturnType<typeof makeQueryClient>
const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>
const view = (data: GitViewChanges | undefined = changes(), active = true) => (
  <CommitBox projectId="p1" checkoutPath="/co" changes={data} active={active} />
)
const renderBox = (data?: GitViewChanges, active = true) => render(view(data, active), { wrapper })

const messageBox = () => screen.getByRole('textbox', { name: 'Commit message' }) as HTMLTextAreaElement
const button = (name: string) => screen.getByRole('button', { name }) as HTMLButtonElement

beforeEach(() => {
  qc = makeQueryClient()
  for (const mocks of [gitWrite, acp, settings, gitView]) {
    Object.values(mocks).forEach((fn) => fn.mockReset().mockResolvedValue(undefined))
  }
  settings.get.mockResolvedValue(null)
  settings.set.mockResolvedValue(undefined)
  acp.agents.mockResolvedValue([devin])
  acp.status.mockResolvedValue({ devin: { state: 'connected', message: null } })
  gitWrite.commit.mockResolvedValue({ head: 'def5678', branch: 'main' })
  gitWrite.push.mockResolvedValue({ summary: 'abc1234..def5678 main -> main' })
  acp.generateCommitMessage.mockResolvedValue('generated subject')
})

describe('CommitBox', () => {
  it('enables Commit only with staged changes, no conflicts, and a message', async () => {
    const { rerender } = renderBox(changes({ staged: [] }))
    fireEvent.change(messageBox(), { target: { value: 'fix it' } })
    expect(button('Commit').disabled).toBe(true)

    rerender(view(changes({ conflicts: [{ path: 'c.ts', orig_path: null, status: 'unmerged' }] })))
    expect(button('Commit').disabled).toBe(true)

    rerender(view())
    expect(button('Commit').disabled).toBe(false)

    fireEvent.change(messageBox(), { target: { value: '   ' } })
    expect(button('Commit').disabled).toBe(true)
  })

  it('commits on Cmd+Enter, clears the box, and reports the result', async () => {
    renderBox()
    const area = messageBox()
    fireEvent.change(area, { target: { value: 'fix the bug' } })
    fireEvent.keyDown(area, { key: 'Enter', metaKey: true })
    await waitFor(() => expect(gitWrite.commit).toHaveBeenCalledWith('p1', '/co', 'fix the bug'))
    await waitFor(() => expect(area.value).toBe(''))
    expect(await screen.findByText('Committed def5678 on main')).toBeTruthy()
  })

  it('reports a detached HEAD after commit', async () => {
    gitWrite.commit.mockResolvedValue({ head: 'abc1234', branch: null })
    renderBox()
    fireEvent.change(messageBox(), { target: { value: 'msg' } })
    fireEvent.click(button('Commit'))
    expect(await screen.findByText('Committed abc1234 on detached HEAD')).toBeTruthy()
  })

  it('keeps the message and shows the alert when commit fails', async () => {
    gitWrite.commit.mockRejectedValue({ code: 'internal', message: 'hook rejected the commit', hint: null })
    renderBox()
    fireEvent.change(messageBox(), { target: { value: 'keep me' } })
    fireEvent.click(button('Commit'))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('hook rejected the commit')
    expect(messageBox().value).toBe('keep me')
  })

  it('labels Push by ahead count and disables it at zero', async () => {
    const { rerender } = renderBox(changes({ ahead: 0 }))
    expect(button('Push').disabled).toBe(true)

    rerender(view(changes({ ahead: 3 })))
    expect(button('Push ↑3').disabled).toBe(false)

    rerender(view(changes({ ahead: null })))
    expect(button('Push new branch').disabled).toBe(false)
  })

  it('shows Git\'s summary after a push and its error on failure', async () => {
    renderBox(changes({ ahead: 2 }))
    fireEvent.click(button('Push ↑2'))
    await waitFor(() => expect(gitWrite.push).toHaveBeenCalledWith('p1', '/co'))
    expect(await screen.findByText('abc1234..def5678 main -> main')).toBeTruthy()

    gitWrite.push.mockRejectedValue({ code: 'internal', message: 'rejected: non-fast-forward', hint: null })
    fireEvent.click(button('Push ↑2'))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('rejected: non-fast-forward')
  })

  it('fills an empty message box from the connected agent', async () => {
    renderBox()
    await waitFor(() => expect(button('Generate').disabled).toBe(false))
    fireEvent.click(button('Generate'))
    await waitFor(() => expect(messageBox().value).toBe('generated subject'))
    expect(acp.generateCommitMessage).toHaveBeenCalledWith('p1', '/co', 'devin')
  })

  it('discards a generate result when the message was edited meanwhile', async () => {
    let resolveGenerate!: (reply: string) => void
    acp.generateCommitMessage.mockImplementation(
      () => new Promise<string>((resolve) => { resolveGenerate = resolve }),
    )
    renderBox()
    await waitFor(() => expect(button('Generate').disabled).toBe(false))
    fireEvent.click(button('Generate'))
    expect(await screen.findByText('Asking Devin…')).toBeTruthy()

    fireEvent.change(messageBox(), { target: { value: 'typed meanwhile' } })
    await act(async () => { resolveGenerate('agent reply') })
    expect(
      await screen.findByText('The message was edited while generating — the result was discarded.'),
    ).toBeTruthy()
    expect(messageBox().value).toBe('typed meanwhile')
  })

  it('Cancel calls acp_cancel and a cancelled rejection stays silent', async () => {
    let rejectGenerate!: (error: unknown) => void
    acp.generateCommitMessage.mockImplementation(
      () => new Promise<string>((_resolve, reject) => { rejectGenerate = reject }),
    )
    renderBox()
    await waitFor(() => expect(button('Generate').disabled).toBe(false))
    fireEvent.click(button('Generate'))
    fireEvent.click(await screen.findByRole('button', { name: 'Cancel' }))
    expect(acp.cancel).toHaveBeenCalledWith('p1', '/co')

    await act(async () => {
      rejectGenerate({ code: 'internal', message: 'cancelled', hint: null })
    })
    await waitFor(() => expect(screen.getByRole('button', { name: 'Generate' })).toBeTruthy())
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('shows a generate failure in the alert', async () => {
    acp.generateCommitMessage.mockRejectedValue({ code: 'internal', message: 'agent exploded', hint: null })
    renderBox()
    await waitFor(() => expect(button('Generate').disabled).toBe(false))
    fireEvent.click(button('Generate'))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('agent exploded')
  })

  it('cancels a running generation on unmount', async () => {
    acp.generateCommitMessage.mockImplementation(() => new Promise<string>(() => {}))
    const { unmount } = renderBox()
    await waitFor(() => expect(button('Generate').disabled).toBe(false))
    fireEvent.click(button('Generate'))
    await screen.findByRole('button', { name: 'Cancel' })
    unmount()
    await waitFor(() => expect(acp.cancel).toHaveBeenCalledWith('p1', '/co'))
  })

  it('Connect shows Connecting… then enables Generate once connected', async () => {
    acp.status
      .mockResolvedValueOnce({ devin: { state: 'disconnected', message: null } })
      .mockResolvedValue({ devin: { state: 'connected', message: null } })
    let resolveConnect!: () => void
    acp.connect.mockImplementation(
      () => new Promise<void>((resolve) => { resolveConnect = resolve }),
    )
    renderBox()
    const connect = await screen.findByRole('button', { name: 'Connect' })
    expect(button('Generate').disabled).toBe(true)

    fireEvent.click(connect)
    expect(await screen.findByRole('button', { name: 'Connecting…' })).toBeTruthy()
    expect(acp.connect).toHaveBeenCalledWith('devin')

    await act(async () => { resolveConnect() })
    await waitFor(() => expect(button('Generate').disabled).toBe(false))
    expect(screen.queryByRole('button', { name: 'Connect' })).toBeNull()
  })

  it('keeps Generate disabled while the agent is disconnected', async () => {
    acp.status.mockResolvedValue({ devin: { state: 'disconnected', message: null } })
    renderBox()
    await screen.findByRole('button', { name: 'Connect' })
    expect(button('Generate').disabled).toBe(true)
  })

  it('shows the agent error as the Connect title and keeps Generate disabled', async () => {
    acp.status.mockResolvedValue({ devin: { state: 'error', message: 'spawn failed' } })
    renderBox()
    const connect = await screen.findByRole('button', { name: 'Connect' })
    expect(connect.getAttribute('title')).toBe('spawn failed')
    expect(button('Generate').disabled).toBe(true)
  })

  it('shows the setup hint when no agent is usable', async () => {
    acp.agents.mockResolvedValue([{ ...devin, enabled: false }])
    renderBox()
    expect(await screen.findByText('Set up an ACP agent in Settings')).toBeTruthy()
    expect(screen.getByLabelText('Commit message agent').textContent).toContain('No ACP agent')
  })

  it('prefers the stored agent while it stays enabled', async () => {
    acp.agents.mockResolvedValue([devin, claude])
    acp.status.mockResolvedValue({
      devin: { state: 'connected', message: null },
      claude: { state: 'connected', message: null },
    })
    settings.get.mockImplementation((key: string) =>
      Promise.resolve(key === 'commit.agent' ? 'claude' : null),
    )
    renderBox()
    await waitFor(() => expect(button('Generate').disabled).toBe(false))
    fireEvent.click(button('Generate'))
    await waitFor(() => expect(acp.generateCommitMessage).toHaveBeenCalledWith('p1', '/co', 'claude'))
  })

  it('falls back to the first connected agent', async () => {
    acp.agents.mockResolvedValue([devin, claude])
    acp.status.mockResolvedValue({
      devin: { state: 'disconnected', message: null },
      claude: { state: 'connected', message: null },
    })
    settings.get.mockImplementation((key: string) =>
      Promise.resolve(key === 'commit.agent' ? 'ghost' : null),
    )
    renderBox()
    await waitFor(() => expect(button('Generate').disabled).toBe(false))
    fireEvent.click(button('Generate'))
    await waitFor(() => expect(acp.generateCommitMessage).toHaveBeenCalledWith('p1', '/co', 'claude'))
  })
})
