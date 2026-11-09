import { pasteToSession, TerminalController, textareaEdit, type TerminalLinks } from './TerminalController'

type FakeCell = { chars: string; width: number }
type FakeLink = {
  range: { start: { x: number; y: number }; end: { x: number; y: number } }
  text: string
  activate: (event: MouseEvent, text: string) => void
}
type FakeLinkProvider = {
  provideLinks: (y: number, callback: (links: FakeLink[] | undefined) => void) => void
}

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
    textarea = document.createElement('textarea')
    constructor(public options: Record<string, unknown>) {
      state.terms.push(this)
    }
    loadAddon(a: { activate(t: FakeTerminal): void }) {
      a.activate(this)
    }
    open = vi.fn((el: HTMLElement) => el.append(this.textarea)) // xterm's textarea lives inside the element
    attachCustomKeyEventHandler = vi.fn()
    pasted: string[] = []
    focus = vi.fn()
    paste(d: string) {
      this.pasted.push(d)
    }
    onData(cb: (d: string) => void) {
      this.dataListeners.add(cb)
      return { dispose: () => void this.dataListeners.delete(cb) }
    }
    write(d: unknown, cb?: () => void) {
      this.written.push(d)
      // Like xterm, parsing finishes before the callback — synchronously is a valid fast case.
      cb?.()
    }
    linkProviders: FakeLinkProvider[] = []
    lines = new Map<number, string | FakeCell[]>()
    providerDisposed = false
    registerLinkProvider = vi.fn((provider: FakeLinkProvider) => {
      this.linkProviders.push(provider)
      return {
        dispose: vi.fn(() => {
          this.providerDisposed = true
        }),
      }
    })
    buffer = {
      active: {
        getLine: (y: number) => {
          const row = this.lines.get(y)
          if (row === undefined) return undefined
          const cells: FakeCell[] = typeof row === 'string' ? [...row].map((chars) => ({ chars, width: 1 })) : row
          return {
            length: cells.length,
            translateToString: (trimRight?: boolean) => {
              let text = ''
              for (let x = 0; x < cells.length; ) {
                text += cells[x].chars || ' '
                x += cells[x].width || 1
              }
              return trimRight ? text.replace(/\s+$/, '') : text
            },
            getCell: (x: number) => {
              if (x < 0 || x >= cells.length) return undefined
              const cell = cells[x]
              return { getChars: () => cell.chars, getWidth: () => cell.width }
            },
          }
        },
      },
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
  class FakeWebgl {
    static failOnCreate = false
    static instances: FakeWebgl[] = []
    lost?: () => void
    disposed = false
    constructor() {
      if (FakeWebgl.failOnCreate) throw new Error('WebGL2 not supported')
      FakeWebgl.instances.push(this)
    }
    activate() {}
    onContextLoss(cb: () => void) {
      this.lost = cb
    }
    dispose() {
      this.disposed = true
    }
  }
  return { state, FakeTerminal, FakeFit, FakeResizeObserver, FakeWebgl }
})
vi.mock('@xterm/xterm', () => ({ Terminal: h.FakeTerminal }))
vi.mock('@xterm/addon-fit', () => ({ FitAddon: h.FakeFit }))
vi.mock('@xterm/addon-webgl', () => ({ WebglAddon: h.FakeWebgl }))

const { state } = h
let onBytes: (seq: number, data: Uint8Array) => void
let unsub: ReturnType<typeof vi.fn>
const service = {
  resize: vi.fn(async () => void state.calls.push('resize')),
  write: vi.fn(async (_id: string, _data: string) => {}),
  ack: vi.fn(async (_id: string, _uptoSeq: number) => {}),
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
/** Writes are queued, so one macrotask lets every pending invoke start. */
const flush = () => new Promise((resolve) => setTimeout(resolve, 0))
const written = () => service.write.mock.calls.map((call) => call[1] as string)

beforeEach(() => {
  state.terms.length = 0
  state.observers.length = 0
  state.calls.length = 0
  h.FakeWebgl.failOnCreate = false
  h.FakeWebgl.instances.length = 0
  delete document.documentElement.dataset.theme
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
    await flush()
    expect(service.write).toHaveBeenCalledWith('s1', 'ls\r')
    c.dispose()
  })

  it('preserves Vietnamese input strings and output bytes', async () => {
    const c = new TerminalController('vi', service)
    await c.mount(element())
    const vietnamese = 'Tiếng Việt: ă â ê ô ơ ư đ a\u0306'
    state.terms[0].dataListeners.forEach((cb) => cb(vietnamese))
    onBytes(0, bytes(vietnamese))
    await flush()

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

  it('recolors a mounted terminal when the app theme changes', async () => {
    const c = new TerminalController('s1', service)
    await c.mount(element())
    const before = state.terms[0].options.theme

    document.documentElement.dataset.theme = 'dracula'
    await flush()
    expect(state.terms[0].options.theme).not.toBe(before)
    expect(state.terms[0].options.theme).toHaveProperty('background')
    c.dispose()
  })

  it('stops recoloring after dispose', async () => {
    const c = new TerminalController('s1', service)
    await c.mount(element())
    c.dispose()
    const before = state.terms[0].options.theme

    document.documentElement.dataset.theme = 'dracula'
    await flush()
    expect(state.terms[0].options.theme).toBe(before)
  })
})

describe('TerminalController renderer', () => {
  it('draws with WebGL and drops it when the context is lost', async () => {
    const c = new TerminalController('gl', service)
    await c.mount(element())
    const [webgl] = h.FakeWebgl.instances
    expect(webgl.disposed).toBe(false)
    webgl.lost?.()
    expect(webgl.disposed).toBe(true)
    c.dispose()
  })

  it('frees the canvas of a hidden terminal and draws again once shown', async () => {
    const c = new TerminalController('gl', service)
    await c.mount(element())
    const [first] = h.FakeWebgl.instances
    c.setVisible(false)
    expect(first.disposed).toBe(true)
    c.setVisible(true)
    expect(h.FakeWebgl.instances).toHaveLength(2)
    expect(h.FakeWebgl.instances[1].disposed).toBe(false)
    c.dispose()
  })

  it('creates no canvas for a terminal mounted hidden until it is shown', async () => {
    const c = new TerminalController('gl', service)
    await c.mount(element(), false)
    expect(h.FakeWebgl.instances).toHaveLength(0)
    c.setVisible(true)
    expect(h.FakeWebgl.instances).toHaveLength(1)
    c.dispose()
  })

  it('recovers a lost context on the next show', async () => {
    const c = new TerminalController('gl', service)
    await c.mount(element())
    h.FakeWebgl.instances[0].lost?.()
    c.setVisible(false)
    c.setVisible(true)
    expect(h.FakeWebgl.instances).toHaveLength(2)
    c.dispose()
  })

  it('keeps working on the DOM renderer when WebGL is unavailable', async () => {
    h.FakeWebgl.failOnCreate = true
    const c = new TerminalController('nogl', service)
    await c.mount(element())
    onBytes(0, bytes('hi'))
    expect(text(state.terms[0])).toBe('hi')
    c.dispose()
  })
})

describe('pasteToSession', () => {
  it('pastes into the mounted terminal of that session, and into none after dispose', async () => {
    const c = new TerminalController('p1', service)
    await c.mount(element())
    expect(pasteToSession('p1', '/a\\ b.png ')).toBe(true)
    expect(state.terms[0].pasted).toEqual(['/a\\ b.png '])
    expect(state.terms[0].focus).toHaveBeenCalled()
    expect(pasteToSession('other', 'x')).toBe(false)

    c.dispose()
    expect(pasteToSession('p1', 'x')).toBe(false)
  })

  it('keeps the newer controller registered when an older one for the same session is disposed', async () => {
    const older = new TerminalController('p2', service)
    await older.mount(element())
    const newer = new TerminalController('p2', service)
    await newer.mount(element())

    older.dispose()
    expect(pasteToSession('p2', 'x')).toBe(true)
    expect(state.terms[1].pasted).toEqual(['x'])
    newer.dispose()
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
})

describe('textareaEdit', () => {
  it('erases the changed tail and types the new one', () => {
    expect(textareaEdit('', 't')).toBe('t')
    expect(textareaEdit('tiê', 'tiên')).toBe('n')
    expect(textareaEdit('tiêng', 'tiếng')).toBe('\x7f\x7f\x7fếng')
    expect(textareaEdit('đây v', 'đây vi')).toBe('i')
    expect(textareaEdit('d', 'đ')).toBe('\x7fđ')
  })

  it('counts characters, not UTF-16 units, and compares NFC', () => {
    expect(textareaEdit('a😀', 'ab')).toBe('\x7fb')
    expect(textareaEdit('tie\u0302', 'tiê')).toBe('')
  })
})

describe('TerminalController IME', () => {
  /** Event order recorded from macOS Simple Telex in WKWebView: the edit lands before the 229 keydown. */
  const imeKey = (ta: HTMLTextAreaElement, value: string, inputType = 'insertReplacementText') => {
    const data = inputType === 'insertText' ? value.slice(ta.value.length) : value
    ta.dispatchEvent(new InputEvent('beforeinput', { inputType, data, bubbles: true }))
    ta.value = value
    ta.dispatchEvent(new InputEvent('input', { inputType, data, bubbles: true }))
  }
  const keyEvent = (ta: HTMLTextAreaElement, type: string, key = ' ') =>
    ta.dispatchEvent(new KeyboardEvent(type, { key, bubbles: true }))
  /** What xterm forwards for a key it handles itself. */
  const xtermData = (data: string) => state.terms[0].dataListeners.forEach((cb) => cb(data))
  const mounted = async () => {
    const c = new TerminalController('s1', service)
    await c.mount(element())
    return { c, ta: state.terms[0].textarea }
  }

  it('forwards each Telex rewrite as its edit, once', async () => {
    const { c, ta } = await mounted()
    imeKey(ta, 'd', 'insertText')
    imeKey(ta, 'đ')
    imeKey(ta, 'đa')
    imeKey(ta, 'đâ')
    await flush()
    expect(written()).toEqual(['d', '\x7fđ', 'a', '\x7fâ'])
    c.dispose()
  })

  it('drops the insertText of a key xterm already sent on keypress', async () => {
    const { c, ta } = await mounted()
    imeKey(ta, 'đây', 'insertText')
    keyEvent(ta, 'keydown')
    keyEvent(ta, 'keypress')
    xtermData(' ') // xterm sent the space on keypress and clears the textarea
    imeKey(ta, ' ', 'insertText') // the IME still inserts it
    keyEvent(ta, 'keyup')
    imeKey(ta, ' v', 'insertText')
    await flush()
    expect(written()).toEqual(['đây', ' ', 'v'])
    c.dispose()
  })

  it('keeps the next letter when it lands before the space is released', async () => {
    const { c, ta } = await mounted()
    keyEvent(ta, 'keydown')
    keyEvent(ta, 'keypress')
    xtermData(' ')
    imeKey(ta, ' ', 'insertText')
    imeKey(ta, ' v', 'insertText') // fast typing: no keyup yet
    imeKey(ta, ' vi')
    await flush()
    expect(written()).toEqual([' ', 'v', 'i'])
    c.dispose()
  })

  it('only drops the keypressed text itself', async () => {
    const { c, ta } = await mounted()
    keyEvent(ta, 'keydown', '1')
    keyEvent(ta, 'keypress', '1')
    xtermData('1') // a cancelled keypress inserts nothing
    imeKey(ta, 'a', 'insertText')
    keyEvent(ta, 'keydown', '1')
    imeKey(ta, 'a1', 'insertText') // a later, unrelated insertText is not dropped
    await flush()
    expect(written()).toEqual(['1', 'a', '1'])
    c.dispose()
  })

  it('starts the next word from an empty textarea after a key xterm handled', async () => {
    const { c, ta } = await mounted()
    imeKey(ta, 'việt', 'insertText')
    xtermData('\x7f') // Backspace goes through xterm, not the IME
    expect(ta.value).toBe('')
    imeKey(ta, 'a', 'insertText')
    await flush()
    expect(written()).toEqual(['việt', '\x7f', 'a'])
    c.dispose()
  })

  it('leaves compositions and other inputs to xterm, and turns off its 229 fallback', async () => {
    const { c, ta } = await mounted()
    const xtermInput = vi.fn()
    ta.addEventListener('input', xtermInput)
    ta.dispatchEvent(new InputEvent('input', { inputType: 'insertCompositionText', isComposing: true, bubbles: true }))
    ta.dispatchEvent(new InputEvent('input', { inputType: 'deleteContentBackward', bubbles: true }))
    imeKey(ta, 'x', 'insertText')
    await flush()
    expect(xtermInput).toHaveBeenCalledTimes(2) // the IME text never reaches xterm's own handler
    expect(written()).toEqual(['x'])
    const handler = state.terms[0].attachCustomKeyEventHandler.mock.calls[0][0] as (e: KeyboardEvent) => boolean
    const keydown = (keyCode: number) => Object.defineProperty(new KeyboardEvent('keydown'), 'keyCode', { value: keyCode })
    expect(handler(keydown(229))).toBe(false)
    expect(handler(keydown(65))).toBe(true)
    c.dispose()
  })

  it('stops listening on dispose', async () => {
    const { c, ta } = await mounted()
    c.dispose()
    imeKey(ta, 'x', 'insertText')
    await flush()
    expect(written()).toEqual([])
  })

  it('keeps queued writes in order', async () => {
    const c = new TerminalController('s1', service)
    await c.mount(element())
    let release!: () => void
    service.write.mockImplementationOnce(() => new Promise<void>((resolve) => (release = resolve)))

    xtermData('a')
    xtermData('b')
    await flush()
    expect(service.write).toHaveBeenCalledTimes(1)

    release()
    await flush()
    expect(written()).toEqual(['a', 'b'])
    c.dispose()
  })
})

describe('TerminalController acks', () => {
  const K64 = 64 * 1024

  it('sends one ack per 64 KiB parsed, with the absolute end offset', async () => {
    const c = new TerminalController('s1', service)
    await c.mount(element())

    onBytes(0, new Uint8Array(K64 - 1))
    expect(service.ack).not.toHaveBeenCalled()

    // Crossing the threshold once acks exactly once, at the end of the last parsed chunk.
    onBytes(K64 - 1, new Uint8Array(2))
    expect(service.ack).toHaveBeenCalledTimes(1)
    expect(service.ack).toHaveBeenCalledWith('s1', K64 + 1)

    // The counter restarts below the threshold.
    onBytes(K64 + 1, new Uint8Array(100))
    expect(service.ack).toHaveBeenCalledTimes(1)
    onBytes(K64 + 101, new Uint8Array(K64 - 100))
    expect(service.ack).toHaveBeenCalledTimes(2)
    expect(service.ack).toHaveBeenLastCalledWith('s1', 2 * K64 + 1)
    c.dispose()
  })

  it('one big chunk still sends a single ack', async () => {
    const c = new TerminalController('s1', service)
    await c.mount(element())
    onBytes(0, new Uint8Array(3 * K64 + 7))
    expect(service.ack).toHaveBeenCalledTimes(1)
    expect(service.ack).toHaveBeenCalledWith('s1', 3 * K64 + 7)
    c.dispose()
  })

  it('ignores an ack failure and keeps parsing', async () => {
    service.ack.mockRejectedValueOnce(new Error('daemon gone'))
    const c = new TerminalController('s1', service)
    await c.mount(element())
    onBytes(0, new Uint8Array(K64))
    onBytes(K64, new Uint8Array(K64))
    expect(service.ack).toHaveBeenCalledTimes(2)
    expect(service.ack).toHaveBeenLastCalledWith('s1', 2 * K64)
    expect(text(state.terms[0])).not.toContain('[goko]')
    c.dispose()
  })

  it('a hidden pane keeps parsing and acking', async () => {
    const c = new TerminalController('hidden', service)
    await c.mount(element(), false)
    onBytes(0, new Uint8Array(K64))
    expect(service.ack).toHaveBeenCalledWith('hidden', K64)
    c.dispose()
  })

  it('sends nothing after dispose', async () => {
    const c = new TerminalController('s1', service)
    await c.mount(element())
    c.dispose()
    onBytes(0, new Uint8Array(3 * K64))
    expect(service.ack).not.toHaveBeenCalled()
  })
})

describe('TerminalController links', () => {
  const mount = async (links?: TerminalLinks) => {
    const c = new TerminalController('s1', service, links)
    await c.mount(element())
    return c
  }
  /** provideLinks' line number is 1-based; the fake buffer's getLine is 0-based like the real API. */
  const provide = async (row: string | FakeCell[], y = 1) => {
    state.terms[0].lines.set(y - 1, row)
    let result: FakeLink[] | undefined
    state.terms[0].linkProviders[0].provideLinks(y, (links) => (result = links))
    await flush()
    return result
  }
  const click = (ctrlKey: boolean) => new MouseEvent('click', { ctrlKey })
  /** One width-1 cell per character. */
  const ascii = (text: string): FakeCell[] => [...text].map((chars) => ({ chars, width: 1 }))
  /** A wide glyph plus the zero-width spacer cell xterm stores after it. */
  const wide = (chars: string): FakeCell[] => [
    { chars, width: 2 },
    { chars: '', width: 0 },
  ]

  it('registers no provider without links', async () => {
    await mount()
    expect(state.terms[0].registerLinkProvider).not.toHaveBeenCalled()
  })

  it('registers one provider for URLs and resolved files on the same row', async () => {
    const openUrl = vi.fn()
    const resolve = vi.fn(async (raw: string) => (raw === 'src/a.ts' ? 'src/a.ts' : null))
    const open = vi.fn()
    await mount({ openUrl, file: { resolve, open } })
    expect(state.terms[0].registerLinkProvider).toHaveBeenCalledTimes(1)

    const found = await provide('see https://example.com/x and src/a.ts:10 missing.ts')
    expect(found).toHaveLength(2)
    expect(found?.[0].text).toBe('https://example.com/x')
    expect(found?.[0].range).toEqual({ start: { x: 5, y: 1 }, end: { x: 25, y: 1 } })
    expect(found?.[1].text).toBe('src/a.ts:10')
    expect(resolve).toHaveBeenCalledWith('src/a.ts')
    expect(resolve).toHaveBeenCalledWith('missing.ts')
  })

  it('maps link ranges to cell columns when wide glyphs precede a URL', async () => {
    const openUrl = vi.fn()
    await mount({ openUrl })
    const found = await provide([...wide('你'), ...wide('好'), { chars: ' ', width: 1 }, ...ascii('https://example.com/x')])
    expect(found).toHaveLength(1)
    expect(found?.[0].text).toBe('https://example.com/x')
    // 你 and 好 each take two cells, so the URL covers columns 6-26, not string columns 4-24.
    expect(found?.[0].range).toEqual({ start: { x: 6, y: 1 }, end: { x: 26, y: 1 } })
  })

  it('opens an https URL only on a modified click', async () => {
    const openUrl = vi.fn()
    await mount({ openUrl })
    const found = await provide('go to https://example.com/docs now')
    const link = found![0]
    expect(link.text).toBe('https://example.com/docs')

    link.activate(click(false), link.text)
    expect(openUrl).not.toHaveBeenCalled()
    link.activate(click(true), link.text)
    expect(openUrl).toHaveBeenCalledWith('https://example.com/docs')
  })

  it('provides no links for file: or javascript: text', async () => {
    const openUrl = vi.fn()
    const resolve = vi.fn(async () => null)
    await mount({ openUrl, file: { resolve, open: vi.fn() } })
    expect(await provide('file:///tmp/a.ts javascript:alert(1)')).toBeUndefined()
    expect(openUrl).not.toHaveBeenCalled()
  })

  it('ignores a malformed https URL without throwing', async () => {
    const openUrl = vi.fn()
    await mount({ openUrl })
    const found = await provide('see https://[ broken')
    const link = found![0]
    expect(link.text).toBe('https://[')

    expect(() => link.activate(click(true), link.text)).not.toThrow()
    expect(openUrl).not.toHaveBeenCalled()
  })

  it('does not resolve files on a URL-only row', async () => {
    const resolve = vi.fn(async () => 'src/a.ts')
    await mount({ openUrl: vi.fn(), file: { resolve, open: vi.fn() } })
    const found = await provide('only https://example.com/x here')
    expect(found).toHaveLength(1)
    expect(resolve).not.toHaveBeenCalled()
  })

  it('reports a file link only for candidates that resolve', async () => {
    const resolve = vi.fn(async (raw: string) => (raw === 'src/a.ts' ? 'src/a.ts' : null))
    await mount({ openUrl: vi.fn(), file: { resolve, open: vi.fn() } })
    const found = await provide('src/a.ts missing.ts')
    expect(found).toHaveLength(1)
    expect(found?.[0].text).toBe('src/a.ts')
  })

  it('keeps the URL link when file resolution rejects', async () => {
    const openUrl = vi.fn()
    const resolve = vi.fn(async () => {
      throw new Error('file list failed')
    })
    const open = vi.fn()
    await mount({ openUrl, file: { resolve, open } })

    const found = await provide('see https://example.com/x and src/a.ts')
    expect(resolve).toHaveBeenCalledWith('src/a.ts')
    expect(found).toHaveLength(1)
    expect(found?.[0].text).toBe('https://example.com/x')

    found![0].activate(click(true), found![0].text)
    expect(openUrl).toHaveBeenCalledWith('https://example.com/x')
    expect(open).not.toHaveBeenCalled()
  })

  it('opens a resolved file at its line only on a modified click', async () => {
    const resolve = vi.fn(async () => 'src/App.tsx')
    const open = vi.fn()
    await mount({ openUrl: vi.fn(), file: { resolve, open } })
    const found = await provide('src/App.tsx:10')
    const link = found![0]
    expect(link.text).toBe('src/App.tsx:10')

    link.activate(click(false), link.text)
    expect(open).not.toHaveBeenCalled()
    link.activate(click(true), link.text)
    expect(open).toHaveBeenCalledWith('src/App.tsx', 10)
  })

  it('reports no links when every candidate misses, and plain rows never resolve', async () => {
    const resolve = vi.fn(async () => null)
    await mount({ openUrl: vi.fn(), file: { resolve, open: vi.fn() } })
    expect(await provide('missing.ts')).toBeUndefined()
    expect(await provide('plain words here')).toBeUndefined()
    expect(resolve).toHaveBeenCalledTimes(1)
  })

  it('disposes the link provider with the controller', async () => {
    const c = await mount({ openUrl: vi.fn() })
    expect(state.terms[0].providerDisposed).toBe(false)
    c.dispose()
    expect(state.terms[0].providerDisposed).toBe(true)
  })
})

describe('TerminalController remount', () => {
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
