import { TerminalController } from './TerminalController'

const h = vi.hoisted(() => {
  const state = {
    terms: [] as FakeTerminal[],
    observers: [] as FakeResizeObserver[],
    fitSize: { cols: 100, rows: 30 },
    calls: [] as string[],
  }
  class FakeTerminal {
    cols = 80
    rows = 24
    written: unknown[] = []
    dataListeners = new Set<(d: string) => void>()
    disposed = false
    constructor(public options: Record<string, unknown>) {
      state.terms.push(this)
    }
    loadAddon(a: { activate(t: FakeTerminal): void }) {
      a.activate(this)
    }
    open = vi.fn()
    onData(cb: (d: string) => void) {
      this.dataListeners.add(cb)
      return { dispose: () => void this.dataListeners.delete(cb) }
    }
    write(d: unknown) {
      this.written.push(d)
    }
    dispose() {
      this.disposed = true
    }
  }
  class FakeFit {
    term!: FakeTerminal
    activate(t: FakeTerminal) {
      this.term = t
    }
    fit = vi.fn(() => void Object.assign(this.term, state.fitSize))
    dispose = vi.fn()
  }
  class FakeResizeObserver {
    observing = false
    constructor(public cb: () => void) {
      state.observers.push(this)
    }
    observe() {
      this.observing = true
    }
    disconnect() {
      this.observing = false
    }
  }
  return { state, FakeTerminal, FakeFit, FakeResizeObserver }
})
vi.mock('@xterm/xterm', () => ({ Terminal: h.FakeTerminal }))
vi.mock('@xterm/addon-fit', () => ({ FitAddon: h.FakeFit }))

const { state } = h
let onBytes: (seq: number, data: Uint8Array) => void
let unsub: ReturnType<typeof vi.fn>
const service = {
  resize: vi.fn(async () => void state.calls.push('resize')),
  write: vi.fn(async () => {}),
  subscribe: vi.fn(async (_id: string, cb: typeof onBytes) => {
    state.calls.push('subscribe')
    onBytes = cb
    return unsub as () => void
  }),
}

const element = (w = 800, h = 600) => {
  const el = document.createElement('div')
  Object.defineProperty(el, 'clientWidth', { value: w })
  Object.defineProperty(el, 'clientHeight', { value: h })
  return el
}
const bytes = (s: string) => new TextEncoder().encode(s)
const text = (t: InstanceType<typeof h.FakeTerminal>) => t.written.map((d) => new TextDecoder().decode(d as Uint8Array)).join('')
const resizeCalls = () => service.resize.mock.calls as unknown as [string, number, number][]

beforeEach(() => {
  state.terms.length = 0
  state.observers.length = 0
  state.calls.length = 0
  state.fitSize = { cols: 100, rows: 30 }
  unsub = vi.fn()
  Object.values(service).forEach((f) => f.mockClear())
  service.resize.mockImplementation(async () => void state.calls.push('resize'))
  vi.stubGlobal('ResizeObserver', h.FakeResizeObserver)
})
afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe('TerminalController mount', () => {
  it('opens, fits, sends the fitted size, then subscribes, and wires bytes and input', async () => {
    const c = new TerminalController('s1', service)
    await c.mount(element())

    expect(state.calls).toEqual(['resize', 'subscribe'])
    expect(service.resize).toHaveBeenCalledWith('s1', 100, 30)
    expect(state.terms[0].options).toMatchObject({ fontSize: 13, scrollback: 5000, cursorBlink: false })

    onBytes(0, bytes('hi'))
    expect(text(state.terms[0])).toBe('hi')
    state.terms[0].dataListeners.forEach((cb) => cb('ls\r'))
    expect(service.write).toHaveBeenCalledWith('s1', 'ls\r')
    c.dispose()
  })

  it('preserves Vietnamese input strings and output bytes', async () => {
    const c = new TerminalController('vi', service)
    await c.mount(element())
    const vietnamese = 'Tiếng Việt: ă â ê ô ơ ư đ a\u0306'
    state.terms[0].dataListeners.forEach((cb) => cb(vietnamese))
    onBytes(0, bytes(vietnamese))

    expect(service.write).toHaveBeenCalledWith('vi', vietnamese)
    expect(text(state.terms[0])).toBe(vietnamese)
    c.dispose()
  })

  it('ignores a second mount', async () => {
    const c = new TerminalController('s1', service)
    await c.mount(element())
    await c.mount(element())
    expect(state.terms).toHaveLength(1)
    expect(service.subscribe).toHaveBeenCalledTimes(1)
    c.dispose()
  })

  it('still subscribes when the initial resize is rejected (exited session)', async () => {
    service.resize.mockRejectedValueOnce({ code: 'invalid_argument', message: 'exited', hint: null })
    const c = new TerminalController('s1', service)
    await c.mount(element())
    expect(service.subscribe).toHaveBeenCalledTimes(1)
    c.dispose()
  })

  it('retries a resize after a transient failure', async () => {
    service.resize.mockRejectedValueOnce(new Error('temporary resize failure'))
    const c = new TerminalController('s1', service)
    await c.mount(element())
    service.resize.mockClear()
    vi.useFakeTimers()
    state.fitSize = { cols: 120, rows: 40 }
    state.observers[0].cb()
    await vi.advanceTimersByTimeAsync(101)
    expect(service.resize).toHaveBeenCalledWith('s1', 120, 40)
    c.dispose()
  })

  it('shows a failed subscribe in the terminal instead of throwing', async () => {
    service.subscribe.mockRejectedValueOnce({ code: 'not_found', message: 'no such session', hint: null })
    const c = new TerminalController('s1', service)
    await c.mount(element())
    expect(state.terms[0].written.join('')).toContain('no such session')
    c.dispose()
  })

  it('skips fit and resize for a 0x0 element but still subscribes', async () => {
    const c = new TerminalController('s1', service)
    vi.useFakeTimers()
    const mounted = c.mount(element(0, 0))
    await vi.advanceTimersByTimeAsync(0)
    await mounted
    state.observers[0].cb()
    await vi.advanceTimersByTimeAsync(100)
    expect(service.resize).not.toHaveBeenCalled()
    expect(service.subscribe).toHaveBeenCalledTimes(1)
    c.dispose()
  })

  it('keeps hidden terminal output mounted without resizing until shown', async () => {
    const c = new TerminalController('hidden', service)
    await c.mount(element(), false)
    expect(service.resize).not.toHaveBeenCalled()
    onBytes(0, bytes('buffered'))
    expect(text(state.terms[0])).toBe('buffered')

    c.setVisible(true)
    await vi.waitFor(() => expect(service.resize).toHaveBeenCalledWith('hidden', 100, 30))
    c.dispose()
  })
})

describe('TerminalController resize', () => {
  it('sends one resize for ten rapid observer events', async () => {
    const c = new TerminalController('s1', service)
    await c.mount(element())
    service.resize.mockClear()
    vi.useFakeTimers()
    state.fitSize = { cols: 120, rows: 40 }
    for (let i = 0; i < 10; i++) {
      state.observers[0].cb()
      await vi.advanceTimersByTimeAsync(10)
    }
    expect(service.resize).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(100)
    expect(resizeCalls()).toEqual([['s1', 120, 40]])
    c.dispose()
  })

  it('does not resend an unchanged size', async () => {
    const c = new TerminalController('s1', service)
    await c.mount(element())
    service.resize.mockClear()
    vi.useFakeTimers()
    state.observers[0].cb()
    await vi.advanceTimersByTimeAsync(100)
    expect(service.resize).not.toHaveBeenCalled()
    c.dispose()
  })
})

describe('TerminalController dispose', () => {
  it('leaves no listeners, disposes everything, and unsubscribes once', async () => {
    const c = new TerminalController('s1', service)
    await c.mount(element())
    c.dispose()
    c.dispose()

    const [term] = state.terms
    expect(term.dataListeners.size).toBe(0)
    expect(state.observers.filter((o) => o.observing)).toHaveLength(0)
    expect(term.disposed).toBe(true)
    expect(unsub).toHaveBeenCalledTimes(1)
  })

  it('drops a pending resize timer and late bytes', async () => {
    const c = new TerminalController('s1', service)
    await c.mount(element())
    service.resize.mockClear()
    vi.useFakeTimers()
    state.fitSize = { cols: 120, rows: 40 }
    state.observers[0].cb()
    c.dispose()
    await vi.advanceTimersByTimeAsync(200)
    expect(service.resize).not.toHaveBeenCalled()

    onBytes(0, bytes('late'))
    expect(state.terms[0].written).toHaveLength(0)
  })

  it('unsubscribes as soon as a pending subscribe resolves', async () => {
    let resolve!: (u: () => void) => void
    service.subscribe.mockImplementationOnce(() => new Promise((r) => (resolve = r)))
    const c = new TerminalController('s1', service)
    const mounted = c.mount(element())
    await vi.waitFor(() => expect(service.subscribe).toHaveBeenCalled())

    c.dispose()
    expect(unsub).not.toHaveBeenCalled()
    resolve(unsub as () => void)
    await mounted
    expect(unsub).toHaveBeenCalledTimes(1)
  })

  it('a disposed controller cannot be mounted again, and a new one gets each chunk once', async () => {
    const first = new TerminalController('s1', service)
    await first.mount(element())
    const oldOnBytes = onBytes
    first.dispose()
    await first.mount(element())
    expect(state.terms).toHaveLength(1)

    const second = new TerminalController('s1', service)
    await second.mount(element())
    oldOnBytes(0, bytes('old')) // late delivery for the first subscription
    onBytes(0, bytes('new'))
    expect(text(state.terms[0])).toBe('')
    expect(text(state.terms[1])).toBe('new')
    second.dispose()
  })
})
