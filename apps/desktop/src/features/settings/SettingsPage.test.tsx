import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient, queryKeys } from '../../queryClient'
import type { AcpAgent, AcpStatusMap } from '../../services/types'
import { useUi } from '../../stores/ui'
import { SettingsPage } from './SettingsPage'

const { settings } = vi.hoisted(() => ({ settings: { get: vi.fn(), set: vi.fn() } }))
vi.mock('../../services/settings', () => ({ settingsService: settings }))

const acp = vi.hoisted(() => ({
  agents: vi.fn(),
  setAgents: vi.fn(),
  connect: vi.fn(),
  disconnect: vi.fn(),
  status: vi.fn(),
  generateCommitMessage: vi.fn(),
  cancel: vi.fn(),
}))
vi.mock('../../services/acp', () => ({ acpService: acp }))

const accounts = vi.hoisted(() => ({
  get: vi.fn(),
  add: vi.fn(),
  rename: vi.fn(),
  remove: vi.fn(),
  setActive: vi.fn(),
}))
vi.mock('../../services/accounts', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../services/accounts')>()),
  accountsService: accounts,
}))

const usage = vi.hoisted(() => ({ get: vi.fn() }))
vi.mock('../../services/usage', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../services/usage')>()),
  usageService: usage,
}))

const PRESETS: AcpAgent[] = [
  {
    id: 'devin',
    label: 'Devin',
    enabled: true,
    command: 'DEVIN_PERMISSION_MODE=auto devin acp --model swe-2-flash-max',
    prompt: '',
    available: true,
    is_default: true,
  },
  { id: 'opencode', label: 'OpenCode', enabled: true, command: 'opencode acp', prompt: '', available: true, is_default: true },
  {
    id: 'claude',
    label: 'Claude Code',
    enabled: true,
    command: 'npx -y @zed-industries/claude-agent-acp',
    prompt: '',
    available: true,
    is_default: true,
  },
  { id: 'codex', label: 'Codex', enabled: true, command: 'npx -y @zed-industries/codex-acp', prompt: '', available: true, is_default: true },
  { id: 'gemini', label: 'Gemini', enabled: true, command: 'gemini --experimental-acp', prompt: '', available: true, is_default: true },
]

let qc: QueryClient
const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>
const renderSettings = () => render(<SettingsPage />, { wrapper })
const openAcpSection = async () => {
  fireEvent.click(screen.getByRole('button', { name: 'ACP agents' }))
  return screen.findByRole('group', { name: 'Devin' })
}

beforeEach(() => {
  qc = makeQueryClient()
  settings.get.mockReset().mockResolvedValue(null)
  settings.set.mockReset().mockResolvedValue(undefined)
  acp.agents.mockReset().mockResolvedValue(PRESETS)
  acp.setAgents.mockReset().mockResolvedValue(PRESETS)
  acp.connect.mockReset().mockResolvedValue(undefined)
  acp.disconnect.mockReset().mockResolvedValue(undefined)
  acp.status.mockReset().mockResolvedValue({})
  acp.generateCommitMessage.mockReset().mockResolvedValue('')
  acp.cancel.mockReset().mockResolvedValue(undefined)
  accounts.get.mockReset().mockResolvedValue({
    claude: { active: 'default', items: [{ id: 'default', name: 'Default', dir: '/h/.claude', is_default: true }] },
    codex: { active: 'default', items: [{ id: 'default', name: 'Default', dir: '/h/.codex', is_default: true }] },
  })
  usage.get.mockReset().mockResolvedValue({
    claude: { five_hour: null, seven_day: null, sessions: {}, updated_at: null },
    codex: { primary: null, secondary: null, updated_at: null },
    accounts: {},
  })
  window.localStorage.clear()
  delete document.documentElement.dataset.theme
  useUi.getState().reset()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

/** Radix Select needs DOM APIs jsdom lacks; opening it is pointerdown on the trigger, choose is pointerup on the option. */
const chooseTheme = async (label: string) => {
  fireEvent.pointerDown(await screen.findByRole('combobox', { name: 'Theme' }), {
    button: 0,
    ctrlKey: false,
    pointerType: 'mouse',
  })
  fireEvent.click(await screen.findByRole('option', { name: label }))
}

it('opens the Editor section and writes the Vim switch to editor.vim', async () => {
  settings.get.mockResolvedValue('1')
  renderSettings()
  const vimSwitch = await screen.findByRole('switch', { name: 'Vim mode' })
  await waitFor(() => expect(vimSwitch.getAttribute('aria-checked')).toBe('true'))

  fireEvent.click(vimSwitch)
  await waitFor(() => expect(settings.set).toHaveBeenCalledWith('editor.vim', '0'))
  await waitFor(() => expect(screen.getByRole('switch', { name: 'Vim mode' }).getAttribute('aria-checked')).toBe('false'))

  fireEvent.click(screen.getByRole('switch', { name: 'Vim mode' }))
  await waitFor(() => expect(settings.set).toHaveBeenLastCalledWith('editor.vim', '1'))
})

it('shows the Appearance section and persists the theme choice', async () => {
  window.localStorage.setItem('goko.theme', 'light')
  renderSettings()
  fireEvent.click(screen.getByRole('button', { name: 'Appearance' }))

  const trigger = await screen.findByRole('combobox', { name: 'Theme' })
  expect(trigger.textContent).toContain('Light')
  await waitFor(() => expect(document.documentElement.dataset.theme).toBe('light'))

  await chooseTheme('Dracula')
  expect(window.localStorage.getItem('goko.theme')).toBe('dracula')
  await waitFor(() => expect(document.documentElement.dataset.theme).toBe('dracula'))

  await chooseTheme('Dark+')
  expect(window.localStorage.getItem('goko.theme')).toBe('dark')
  await waitFor(() => expect(document.documentElement.dataset.theme).toBeUndefined())
})

it('resolves the System theme from the OS preference', async () => {
  vi.stubGlobal('matchMedia', () => ({ matches: true, addEventListener: vi.fn() }))
  renderSettings()
  fireEvent.click(screen.getByRole('button', { name: 'Appearance' }))

  await chooseTheme('System')
  expect(window.localStorage.getItem('goko.theme')).toBe('system')
  await waitFor(() => expect(document.documentElement.dataset.theme).toBe('light'))
})

it('keeps the Editor card hidden until its section is selected', async () => {
  renderSettings()
  expect(await screen.findByRole('switch', { name: 'Vim mode' })).toBeTruthy()
  expect(screen.queryByRole('combobox', { name: 'Theme' })).toBeNull()

  fireEvent.click(screen.getByRole('button', { name: 'Appearance' }))
  expect(screen.queryByRole('switch', { name: 'Vim mode' })).toBeNull()
})

it('opens on the Accounts section when settingsSection requests it and clears the value', async () => {
  act(() => useUi.getState().openSettings('accounts'))
  renderSettings()

  expect(await screen.findByRole('group', { name: 'Claude Code accounts' })).toBeTruthy()
  expect(screen.queryByRole('switch', { name: 'Vim mode' })).toBeNull()
  expect(useUi.getState().settingsSection).toBeNull()
})

it('switches to the requested section while Settings is already open', async () => {
  renderSettings()
  expect(await screen.findByRole('switch', { name: 'Vim mode' })).toBeTruthy()

  act(() => useUi.getState().openSettings('accounts'))

  expect(await screen.findByRole('group', { name: 'Claude Code accounts' })).toBeTruthy()
  expect(screen.queryByRole('switch', { name: 'Vim mode' })).toBeNull()
  expect(useUi.getState().settingsSection).toBeNull()
})

it('still starts on Editor when Settings opens the normal way', async () => {
  act(() => useUi.getState().setActiveView('settings'))
  renderSettings()

  expect(await screen.findByRole('switch', { name: 'Vim mode' })).toBeTruthy()
  expect(screen.queryByRole('group', { name: 'Claude Code accounts' })).toBeNull()
})

it('lists Accounts in the nav and shows the Claude Code and Codex cards', async () => {
  renderSettings()
  fireEvent.click(screen.getByRole('button', { name: 'Accounts' }))

  const claude = await screen.findByRole('group', { name: 'Claude Code accounts' })
  expect(await within(claude).findByRole('group', { name: 'Default' })).toBeTruthy()
  const codex = await screen.findByRole('group', { name: 'Codex accounts' })
  expect(await within(codex).findByRole('group', { name: 'Default' })).toBeTruthy()
})

describe('ACP agents section', () => {
  it('opens from the navigation and reads status only while visible', async () => {
    renderSettings()
    expect(screen.queryByRole('group', { name: 'Devin' })).toBeNull()
    expect(acp.status).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole('button', { name: 'ACP agents' }))
    expect(await screen.findByRole('group', { name: 'Devin' })).toBeTruthy()
    await waitFor(() => expect(acp.agents).toHaveBeenCalledTimes(1))
    await waitFor(() => expect(acp.status).toHaveBeenCalled())
  })

  it('renders the five preset agents with their commands', async () => {
    renderSettings()
    await openAcpSection()
    for (const agent of PRESETS) {
      const row = screen.getByRole('group', { name: agent.label })
      const command = within(row).getByLabelText(`${agent.label} command`) as HTMLInputElement
      expect(command.value).toBe(agent.command)
      expect(within(row).getByLabelText(`${agent.label} commit prompt`)).toBeTruthy()
      expect(within(row).getByRole('switch', { name: `${agent.label} enabled` })).toBeTruthy()
      expect(within(row).getByRole('button', { name: 'Connect' })).toBeTruthy()
    }
  })

  it('toggling Enabled persists only the differing override rows', async () => {
    const stored = PRESETS.map((agent) =>
      agent.id === 'opencode' ? { ...agent, enabled: false, is_default: false } : agent,
    )
    acp.agents.mockResolvedValue(stored)
    acp.setAgents.mockResolvedValue(
      stored.map((agent) => (agent.id === 'devin' ? { ...agent, enabled: false, is_default: false } : agent)),
    )
    renderSettings()
    const row = await openAcpSection()

    fireEvent.click(within(row).getByRole('switch', { name: 'Devin enabled' }))
    await waitFor(() =>
      expect(acp.setAgents).toHaveBeenCalledWith([
        { id: 'devin', enabled: false, command: PRESETS[0].command, prompt: '' },
        { id: 'opencode', enabled: false, command: 'opencode acp', prompt: '' },
      ]),
    )
  })

  it('saves a changed command on blur', async () => {
    const saved = PRESETS.map((agent) =>
      agent.id === 'devin' ? { ...agent, command: 'devin acp --fast', is_default: false } : agent,
    )
    acp.setAgents.mockResolvedValue(saved)
    renderSettings()
    const row = await openAcpSection()
    const input = within(row).getByLabelText('Devin command') as HTMLInputElement

    fireEvent.change(input, { target: { value: 'devin acp --fast' } })
    fireEvent.blur(input)
    await waitFor(() =>
      expect(acp.setAgents).toHaveBeenCalledWith([
        { id: 'devin', enabled: true, command: 'devin acp --fast', prompt: '' },
      ]),
    )
    await waitFor(() => expect(input.value).toBe('devin acp --fast'))
    expect(within(row).getByRole('button', { name: 'Reset Devin' })).toBeTruthy()
  })

  it('saves a changed commit prompt on blur', async () => {
    const saved = PRESETS.map((agent) =>
      agent.id === 'gemini' ? { ...agent, prompt: 'Write a terse message.', is_default: false } : agent,
    )
    acp.setAgents.mockResolvedValue(saved)
    renderSettings()
    await openAcpSection()
    const area = within(screen.getByRole('group', { name: 'Gemini' })).getByLabelText(
      'Gemini commit prompt',
    ) as HTMLTextAreaElement

    fireEvent.change(area, { target: { value: 'Write a terse message.' } })
    fireEvent.blur(area)
    await waitFor(() =>
      expect(acp.setAgents).toHaveBeenCalledWith([
        { id: 'gemini', enabled: true, command: 'gemini --experimental-acp', prompt: 'Write a terse message.' },
      ]),
    )
  })

  it('shows a rejected command inline and keeps the typed value', async () => {
    acp.setAgents.mockRejectedValue({
      code: 'invalid_argument',
      message: "Devin's command is invalid: unterminated quote",
      hint: null,
    })
    renderSettings()
    const row = await openAcpSection()
    const input = within(row).getByLabelText('Devin command') as HTMLInputElement

    fireEvent.change(input, { target: { value: 'devin "acp' } })
    fireEvent.blur(input)
    await waitFor(() => expect(acp.setAgents).toHaveBeenCalledTimes(1))
    const alert = await within(row).findByRole('alert')
    expect(alert.textContent).toContain("Devin's command is invalid: unterminated quote")
    expect(input.value).toBe('devin "acp')
  })

  it('Reset to default removes only that agent’s override row', async () => {
    const stored = PRESETS.map((agent) => {
      if (agent.id === 'devin') return { ...agent, command: 'devin acp --custom', is_default: false }
      if (agent.id === 'opencode') return { ...agent, enabled: false, is_default: false }
      return agent
    })
    acp.agents.mockResolvedValue(stored)
    acp.setAgents.mockResolvedValue(PRESETS)
    renderSettings()
    const row = await openAcpSection()

    fireEvent.click(within(row).getByRole('button', { name: 'Reset Devin' }))
    await waitFor(() =>
      expect(acp.setAgents).toHaveBeenCalledWith([
        { id: 'opencode', enabled: false, command: 'opencode acp', prompt: '' },
      ]),
    )
    await waitFor(() => expect(within(row).queryByRole('button', { name: 'Reset Devin' })).toBeNull())
  })

  it('shows Disconnect for a connected agent and disconnects it', async () => {
    acp.status.mockResolvedValue({ devin: { state: 'connected', message: null } })
    renderSettings()
    const row = await openAcpSection()

    await within(row).findByText('connected')
    fireEvent.click(within(row).getByRole('button', { name: 'Disconnect' }))
    await waitFor(() => expect(acp.disconnect).toHaveBeenCalledWith('devin'))
  })

  it('connects a disconnected agent and follows connecting to connected', async () => {
    let statusMap: AcpStatusMap = {}
    acp.status.mockImplementation(() => Promise.resolve(statusMap))
    acp.connect.mockImplementation(async () => {
      statusMap = { devin: { state: 'connecting', message: null } }
    })
    renderSettings()
    const row = await openAcpSection()

    fireEvent.click(await within(row).findByRole('button', { name: 'Connect' }))
    await waitFor(() => expect(acp.connect).toHaveBeenCalledWith('devin'))
    await within(row).findByText('connecting')
    const connectingButton = within(row).getByRole('button', { name: 'Connecting…' })
    expect(connectingButton.getAttribute('disabled')).not.toBeNull()

    statusMap = { devin: { state: 'connected', message: null } }
    await act(async () => {
      await qc.invalidateQueries({ queryKey: queryKeys.acpStatus })
    })
    await within(row).findByText('connected')
    expect(within(row).getByRole('button', { name: 'Disconnect' })).toBeTruthy()
  })

  it('shows an error status with its message and a Connect button', async () => {
    acp.status.mockResolvedValue({ devin: { state: 'error', message: 'devin acp exited unexpectedly' } })
    renderSettings()
    const row = await openAcpSection()

    await within(row).findByText('devin acp exited unexpectedly')
    expect(within(row).getByText('error')).toBeTruthy()
    expect(within(row).getByRole('button', { name: 'Connect' })).toBeTruthy()
  })

  it('disables Connect for disabled and unavailable agents', async () => {
    const stored = PRESETS.map((agent) => {
      if (agent.id === 'opencode') return { ...agent, enabled: false, is_default: false }
      if (agent.id === 'codex') return { ...agent, available: false }
      return agent
    })
    acp.agents.mockResolvedValue(stored)
    renderSettings()
    await openAcpSection()

    const disabledConnect = within(screen.getByRole('group', { name: 'OpenCode' })).getByRole('button', {
      name: 'Connect',
    })
    const missingConnect = within(screen.getByRole('group', { name: 'Codex' })).getByRole('button', { name: 'Connect' })
    expect(disabledConnect.getAttribute('disabled')).not.toBeNull()
    expect(missingConnect.getAttribute('disabled')).not.toBeNull()
    expect(missingConnect.getAttribute('title')).toBe('Not found on PATH')
  })
})
