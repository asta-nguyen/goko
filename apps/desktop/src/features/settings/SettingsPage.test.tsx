import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient } from '../../queryClient'
import { SettingsPage } from './SettingsPage'

const { settings } = vi.hoisted(() => ({ settings: { get: vi.fn(), set: vi.fn() } }))
vi.mock('../../services/settings', () => ({ settingsService: settings }))

let qc: QueryClient
const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>
const renderSettings = () => render(<SettingsPage />, { wrapper })

beforeEach(() => {
  qc = makeQueryClient()
  settings.get.mockReset().mockResolvedValue(null)
  settings.set.mockReset().mockResolvedValue(undefined)
  window.localStorage.clear()
  delete document.documentElement.dataset.theme
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
