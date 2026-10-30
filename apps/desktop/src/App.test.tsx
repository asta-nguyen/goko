import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { makeQueryClient } from './queryClient'
import type { SessionInfo } from './services/types'
import { useUi } from './stores/ui'
import App from './App'

type Handler = (e: { payload: unknown }) => void
const { handlers, listen, service, log } = vi.hoisted(() => {
  const handlers = new Map<string, Set<Handler>>()
  const listen = vi.fn(async (name: string, cb: Handler) => {
    if (!handlers.has(name)) handlers.set(name, new Set())
    handlers.get(name)!.add(cb)
    return () => void handlers.get(name)!.delete(cb)
  })
  return {
    handlers,
    listen,
    log: [] as string[],
    service: { list: vi.fn(), status: vi.fn(), create: vi.fn(), stop: vi.fn() },
  }
})
vi.mock('@tauri-apps/api/event', () => ({ listen }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
vi.mock('./services/session', () => ({ sessionService: service }))
// jsdom has no layout, so the xterm-based controller is replaced; its own tests cover it.
vi.mock('./features/terminal/TerminalController', () => ({
  TerminalController: class {
    constructor(private id: string) {
      log.push(`new:${id}`)
    }
    async mount() {}
    dispose() {
      log.push(`dispose:${this.id}`)
    }
  },
}))

const session = (id: string, over: Partial<SessionInfo> = {}): SessionInfo => ({
  id,
  title: `title-${id}`,
  argv: ['sh'],
  cwd: '/tmp',
  state: 'running',
  exit_code: null,
  error: null,
  started_at: Date.now(),
  ended_at: null,
  ...over,
})
const status = (connected: boolean) => ({ connected, default_shell: '/bin/zsh', home: '/h' })
const listeners = () => [...handlers.values()].reduce((n, set) => n + set.size, 0)
const emit = (name: string, payload: unknown) => act(() => void handlers.get(name)?.forEach((cb) => cb({ payload })))

let qc: QueryClient
const renderApp = () =>
  render(
    <QueryClientProvider client={qc}>
      <App />
    </QueryClientProvider>,
  )

beforeEach(async () => {
  qc = makeQueryClient()
  handlers.clear()
  log.length = 0
  Object.values(service).forEach((f) => f.mockReset())
  service.list.mockResolvedValue([session('a'), session('b')])
  service.status.mockResolvedValue(status(true))
  useUi.setState({ selectedId: null })
})

describe('App', () => {
  it('renders a row per session and mounts a terminal for the one selected', async () => {
    renderApp()
    const list = await screen.findByRole('list')
    expect(within(list).getAllByRole('button')).toHaveLength(2)
    expect(screen.getByText(/select or create a session/i)).toBeTruthy()
    expect(log).toEqual([])

    fireEvent.click(within(list).getByRole('button', { name: /title-b/ }))
    expect(log).toEqual(['new:b'])
    expect(within(list).getByRole('button', { name: /title-b/ }).getAttribute('aria-current')).toBe('true')
    expect(within(list).getByRole('button', { name: /title-a/ }).getAttribute('aria-current')).toBe('false')
    expect(screen.getByRole('button', { name: 'Stop' })).toBeTruthy()

    fireEvent.click(within(list).getByRole('button', { name: /title-a/ }))
    expect(log).toEqual(['new:b', 'dispose:b', 'new:a'])
  })

  it('shows the banner and disables controls while disconnected, and recovers', async () => {
    renderApp()
    await vi.waitFor(() => expect(listeners()).toBe(3))
    fireEvent.click(await screen.findByRole('button', { name: /title-a/ }))
    const stop = screen.getByRole('button', { name: 'Stop' }) as HTMLButtonElement
    expect(stop.disabled).toBe(false)
    expect(screen.queryByRole('alert')).toBeNull()

    emit('daemon://status', { connected: false })
    expect((await screen.findByRole('alert')).textContent).toMatch(/unreachable/i)
    expect(screen.getByRole('status').getAttribute('aria-label')).toBe('Daemon disconnected')
    expect(stop.disabled).toBe(true)

    emit('daemon://status', { connected: true }) // the bridge reconnected: both keys refetch
    await waitFor(() => expect(screen.queryByRole('alert')).toBeNull())
    expect(screen.getByRole('status').getAttribute('aria-label')).toBe('Daemon connected')
    expect(stop.disabled).toBe(false)
  })

  it('applies pushed session events to the list', async () => {
    renderApp()
    await vi.waitFor(() => expect(listeners()).toBe(3))
    await screen.findByRole('list')
    emit('session://updated', session('c'))
    expect(await screen.findByRole('button', { name: /title-c/ })).toBeTruthy()
  })

  it('stops the event sync on unmount', async () => {
    const { unmount } = renderApp()
    await vi.waitFor(() => expect(listeners()).toBe(3))
    unmount()
    await vi.waitFor(() => expect(listeners()).toBe(0))
  })
})
