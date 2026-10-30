import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { open } from '@tauri-apps/plugin-dialog'
import { makeQueryClient } from '../../queryClient'
import type { SessionInfo } from '../../services/types'
import { useUi } from '../../stores/ui'
import { NewSessionForm } from './NewSessionForm'

const { service } = vi.hoisted(() => ({ service: { create: vi.fn(), status: vi.fn() } }))
vi.mock('../../services/session', () => ({ sessionService: service }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
const openDialog = vi.mocked(open)

const session = (over: Partial<SessionInfo> = {}): SessionInfo => ({
  id: 'new',
  title: 'zsh',
  argv: ['/bin/zsh'],
  cwd: '/work',
  state: 'running',
  exit_code: null,
  error: null,
  started_at: 1,
  ended_at: null,
  ...over,
})

let qc: QueryClient
const renderForm = () =>
  render(
    <QueryClientProvider client={qc}>
      <NewSessionForm />
    </QueryClientProvider>,
  )
const start = () => screen.getByRole('button', { name: 'Start' }) as HTMLButtonElement
const chooseDirectory = async (path: string) => {
  openDialog.mockResolvedValueOnce(path)
  fireEvent.click(screen.getByRole('button', { name: /choose directory/i }))
  await screen.findByText(path)
}

beforeEach(() => {
  qc = makeQueryClient()
  service.create.mockReset()
  service.status.mockReset().mockResolvedValue({ connected: true, default_shell: '/bin/zsh', home: '/h' })
  openDialog.mockReset()
  useUi.setState({ selectedId: null })
})

describe('NewSessionForm', () => {
  it('prefills the default shell and keeps Start disabled, with a hint, until a directory is chosen', async () => {
    renderForm()
    expect(await screen.findByDisplayValue('/bin/zsh')).toBeTruthy()
    expect(start().disabled).toBe(true)
    expect(screen.getByText(/choose a directory and enter a command/i)).toBeTruthy()

    await chooseDirectory('/work')
    expect(start().disabled).toBe(false)

    fireEvent.change(screen.getByLabelText('Command'), { target: { value: '   ' } })
    expect(start().disabled).toBe(true)
  })

  it('calls create with command, cwd and 80x24, then selects the new session', async () => {
    service.create.mockResolvedValue(session({ id: 'new' }))
    renderForm()
    await screen.findByDisplayValue('/bin/zsh')
    await chooseDirectory('/work')
    fireEvent.change(screen.getByLabelText('Command'), { target: { value: 'sh -l' } })
    fireEvent.click(start())

    await waitFor(() => expect(useUi.getState().selectedId).toBe('new'))
    expect(service.create.mock.calls[0][0]).toEqual({ command: 'sh -l', cwd: '/work', cols: 80, rows: 24 })
  })

  it('renders the message and hint of a BridgeError', async () => {
    service.create.mockRejectedValue({ code: 'invalid_argument', message: 'unbalanced quote', hint: 'close the quote' })
    renderForm()
    await screen.findByDisplayValue('/bin/zsh')
    await chooseDirectory('/work')
    fireEvent.click(start())

    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('unbalanced quote')
    expect(alert.textContent).toContain('close the quote')
    expect(useUi.getState().selectedId).toBeNull()
  })

  it('calls create once for a fast double-click and disables the form while pending', async () => {
    service.create.mockReturnValue(new Promise(() => {}))
    renderForm()
    await screen.findByDisplayValue('/bin/zsh')
    await chooseDirectory('/work')

    // both clicks land before React re-renders
    await act(async () => {
      start().click()
      start().click()
    })
    expect(service.create).toHaveBeenCalledTimes(1)
    // disabled through the <fieldset>, which the `disabled` property does not reflect
    await waitFor(() => expect(start().matches(':disabled')).toBe(true))
    expect(screen.getByLabelText('Command').matches(':disabled')).toBe(true)
  })

  it('leaves the directory unchanged when the dialog is cancelled', async () => {
    renderForm()
    openDialog.mockResolvedValueOnce(null)
    fireEvent.click(screen.getByRole('button', { name: /choose directory/i }))
    await waitFor(() => expect(openDialog).toHaveBeenCalledWith({ directory: true }))
    expect(screen.getByText('No directory chosen')).toBeTruthy()

    await chooseDirectory('/work')
    openDialog.mockResolvedValueOnce(null)
    fireEvent.click(screen.getByRole('button', { name: /choose directory/i }))
    await waitFor(() => expect(openDialog).toHaveBeenCalledTimes(3))
    expect(screen.getByText('/work')).toBeTruthy()
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('shows an error when the dialog itself fails', async () => {
    renderForm()
    openDialog.mockRejectedValueOnce(new Error('dialog.open not allowed'))
    fireEvent.click(screen.getByRole('button', { name: /choose directory/i }))
    expect((await screen.findByRole('alert')).textContent).toContain('not allowed')
  })
})
