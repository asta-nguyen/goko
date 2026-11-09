import { FitAddon } from '@xterm/addon-fit'
import { WebglAddon } from '@xterm/addon-webgl'
import { Terminal, type IBufferLine, type ILink, type IDisposable } from '@xterm/xterm'
import { goToModifier } from '../../lib/utils'
import { toBridgeError } from '../../services/errors'
import type { SessionService } from '../../services/session'
import { findFileRefs, findUrlRefs } from './terminal-links'

type Service = Pick<SessionService, 'subscribe' | 'write' | 'resize' | 'ack'>

const RESIZE_DEBOUNCE_MS = 100
const ACK_THRESHOLD = 64 * 1024
const BACKSPACE = '\x7f'
const noop = () => {}
// Colours come from the theme tokens (CSS variables), read once at mount.
const token = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim() || undefined

const mounted = new Map<string, TerminalController>()

/** Pastes into a session's terminal (bracketed when the program asked for it). False if none is mounted. */
export const pasteToSession = (id: string, text: string): boolean => {
  const controller = mounted.get(id)
  controller?.paste(text)
  return controller !== undefined
}

/** What the terminal must receive to turn `before` into `after`: erase the changed tail, type the new one. */
export const textareaEdit = (before: string, after: string): string => {
  const old = [...before.normalize('NFC')]
  const next = [...after.normalize('NFC')]
  let common = 0
  while (common < old.length && common < next.length && old[common] === next[common]) common += 1
  return BACKSPACE.repeat(old.length - common) + next.slice(common).join('')
}

/**
 * The 1-based column span of the buffer cell behind each UTF-16 offset of `text`.
 * The scanners report string offsets but link ranges are cell columns, and the two
 * diverge once a wide glyph (CJK, emoji) precedes the link. The walk mirrors
 * `translateToString`'s cell advance so both stay aligned; `first`/`last` are
 * indexed by string offset and hold the first and last 1-based column of its cell.
 */
const cellColumns = (line: IBufferLine, text: string): { first: number[]; last: number[] } => {
  const first: number[] = []
  const last: number[] = []
  let offset = 0
  for (let x = 0; x < line.length && offset < text.length; ) {
    const cell = line.getCell(x)
    if (!cell) break
    const chars = cell.getChars() || ' ' // cells without content translate to a space
    const span = cell.getWidth() || 1
    for (let i = 0; i < chars.length && offset < text.length; i++, offset++) {
      first[offset] = x + 1
      last[offset] = x + span
    }
    x += span
  }
  return { first, last }
}

/** What an activated link does: URLs go to the system browser, checkout files to the Editor. */
export interface TerminalLinks {
  openUrl(url: string): void
  file?: {
    /** The checkout-relative path a printed candidate names, or null when it is not a checkout file. */
    resolve(raw: string): Promise<string | null>
    open(path: string, line?: number): void
  }
}

/** One xterm instance bound to one session. Output goes straight to `term.write`, never through React. */
export class TerminalController {
  private term?: Terminal
  private element?: HTMLElement
  private fit = new FitAddon()
  private webgl?: WebglAddon
  private observer?: ResizeObserver
  private themeObserver?: MutationObserver
  private timer?: ReturnType<typeof setTimeout>
  private onData?: { dispose(): void }
  private imeCleanup?: () => void
  private unsubscribe?: () => void
  private linksDisposable?: IDisposable
  private sent = ''
  private disposed = false
  private visible = true
  private writes: Promise<void> = Promise.resolve()
  private parsed = 0

  constructor(
    private sessionId: string,
    private service: Service,
    private links?: TerminalLinks,
  ) {}

  /** Open, fit, send the fitted size, then subscribe, so the replay never lands in a wrong-size terminal. */
  async mount(element: HTMLElement, visible = true): Promise<void> {
    if (this.term || this.disposed) return
    this.visible = visible
    const { sessionId: id, service } = this
    const term = (this.term = new Terminal({
      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace',
      fontSize: 13,
      scrollback: 5000,
      cursorBlink: false,
      theme: { background: token('--bg'), foreground: token('--text'), cursor: token('--accent') },
    }))
    this.element = element
    mounted.set(id, this)
    term.loadAddon(this.fit)
    term.open(element)
    if (visible) this.useWebgl(term)
    this.attachIme(term, element)
    this.registerLinks(term)
    // Writes to an exited session are rejected by the daemon; the view's banner shows why.
    this.onData = term.onData((data) => {
      // A key xterm handled itself (space, Backspace, Enter, arrows) ends the IME word, and the
      // textarea no longer mirrors the line, so start the next IME edit from empty.
      if (term.textarea) term.textarea.value = ''
      this.send(data)
    })
    this.observer = new ResizeObserver(() => {
      clearTimeout(this.timer)
      this.timer = setTimeout(() => void this.syncSize(), RESIZE_DEBOUNCE_MS)
    })
    this.observer.observe(element)
    // Theme tokens are read at mount; re-read them when the app theme flips.
    this.themeObserver = new MutationObserver(() => this.updateTheme())
    this.themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] })

    if (this.visible) await this.syncSize()
    if (this.disposed) return
    try {
      const unsubscribe = await service.subscribe(id, (seq, data) => {
        if (this.disposed) return
        // The write callback runs once xterm parsed the chunk: that is what the
        // daemon's window needs to hear about, not the bytes handed to `write`.
        term.write(data, () => this.parsedChunk(id, seq, data.length))
      })
      if (this.disposed) unsubscribe() // disposed while subscribing
      else this.unsubscribe = unsubscribe
    } catch (e) {
      if (!this.disposed) term.write(`\r\n[goko] ${toBridgeError(e).message}\r\n`)
    }
  }

  /** Idempotent. */
  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    if (mounted.get(this.sessionId) === this) mounted.delete(this.sessionId)
    clearTimeout(this.timer)
    this.observer?.disconnect()
    this.themeObserver?.disconnect()
    this.imeCleanup?.()
    this.unsubscribe?.()
    this.onData?.dispose()
    this.linksDisposable?.dispose()
    this.fit.dispose()
    this.term?.dispose()
  }

  /**
   * The default DOM renderer rebuilds every row on each full-screen redraw (agent TUIs do dozens a
   * second), and WebKit frees that garbage only on GC: hundreds of MB. WebGL draws to a canvas.
   * Without WebGL, or after its context is lost, xterm falls back to the DOM renderer.
   * Every terminal stays mounted, and each canvas costs tens of MB, so only visible ones keep it.
   */
  private useWebgl(term: Terminal): void {
    try {
      const webgl = new WebglAddon()
      webgl.onContextLoss(() => this.dropWebgl(webgl))
      term.loadAddon(webgl)
      this.webgl = webgl
    } catch {
      // keep the DOM renderer
    }
  }

  private dropWebgl(webgl: WebglAddon): void {
    webgl.dispose()
    if (this.webgl === webgl) this.webgl = undefined
  }

  private updateTheme(): void {
    if (!this.term) return
    this.term.options.theme = { background: token('--bg'), foreground: token('--text'), cursor: token('--accent') }
  }

  /**
   * One provider for HTTP(S) URLs and checkout files. A row's URL candidates never need the file
   * list; only a row with file candidates resolves them through `links.file`, then reports every
   * link at once. Activation stays behind the Editor's Cmd/Ctrl rule in `goToModifier`.
   */
  private registerLinks(term: Terminal): void {
    const links = this.links
    if (!links) return
    this.linksDisposable = term.registerLinkProvider({
      provideLinks: (y, callback) => {
        const line = term.buffer.active.getLine(y - 1)
        const text = line?.translateToString(true)
        if (!line || !text) {
          callback(undefined)
          return
        }
        const urlRefs = findUrlRefs(text)
        const file = links.file
        const fileRefs = file ? findFileRefs(text) : []
        if (urlRefs.length === 0 && fileRefs.length === 0) {
          callback(undefined)
          return
        }
        // `y` is a 1-based buffer line and link ranges are 1-based cell columns;
        // the scanners return UTF-16 string offsets, mapped through the cells.
        const columns = cellColumns(line, text)
        const range = (start: number, end: number) => ({
          start: { x: columns.first[start], y },
          end: { x: columns.last[end - 1], y },
        })
        const urlLinks: ILink[] = urlRefs.map((ref): ILink => ({
          range: range(ref.start, ref.end),
          text: ref.url,
          activate: (event) => {
            if (!goToModifier(event)) return
            try {
              const { protocol } = new URL(ref.url)
              if (protocol !== 'http:' && protocol !== 'https:') return
            } catch {
              return
            }
            links.openUrl(ref.url)
          },
        }))
        if (!file || fileRefs.length === 0) {
          callback(urlLinks.length ? urlLinks : undefined)
          return
        }
        void Promise.all(
          fileRefs.map((ref) =>
            file.resolve(ref.path).then(
              (path) => ({ ref, path }),
              () => ({ ref, path: null }),
            ),
          ),
        ).then((resolved) => {
          if (this.disposed) return
          const fileLinks = resolved.flatMap(({ ref, path }): ILink[] =>
            path === null
              ? []
              : [
                  {
                    range: range(ref.start, ref.end),
                    text: text.slice(ref.start, ref.end),
                    activate: (event) => {
                      if (goToModifier(event)) file.open(path, ref.line)
                    },
                  },
                ],
          )
          const all = [...urlLinks, ...fileLinks]
          callback(all.length ? all : undefined)
        })
      },
    })
  }

  paste(text: string): void {
    this.term?.focus()
    this.term?.paste(text)
  }

  setVisible(visible: boolean): void {
    if (this.disposed || this.visible === visible) return
    this.visible = visible
    if (!visible) {
      if (this.webgl) this.dropWebgl(this.webgl)
      return
    }
    if (this.term && !this.webgl) this.useWebgl(this.term)
    void this.syncSize()
  }

  /**
   * Plain-text input (no composition) is forwarded as the textarea edit it made. macOS Simple
   * Telex in WKWebView fires `input` *before* its keyCode-229 keydown and rewrites the whole word
   * (`insertReplacementText` "tiê" -> "tiên"); xterm's 229 fallback diffs that wrongly (equal
   * length resends everything) and its `insertText` path only sometimes sends. So this owns those
   * input events (capture on the parent, ahead of xterm) and switches the 229 fallback off.
   * Composition IMEs (Japanese, Chinese) still go through xterm untouched.
   */
  private attachIme(term: Terminal, element: HTMLElement): void {
    const textarea = term.textarea
    if (!textarea) return
    let before = ''
    // xterm already sent a key it saw on keypress (the IME's space), yet the IME still inserts it.
    // Drop exactly that insertText. Matching the text, cleared by the next keydown rather than
    // keyup, stays right when keys overlap or keyup lands first.
    let pressed = ''
    const onKeyDown = () => (pressed = '')
    const onKeyPress = (event: Event) => (pressed = (event as KeyboardEvent).key)
    const onBeforeInput = () => (before = textarea.value)
    const onInput = (event: Event) => {
      const { inputType, isComposing, data } = event as InputEvent
      if (event.target !== textarea || isComposing) return
      if (inputType !== 'insertText' && inputType !== 'insertReplacementText') return
      event.stopImmediatePropagation()
      if (inputType === 'insertText' && pressed && data === pressed) {
        pressed = ''
        return
      }
      this.send(textareaEdit(before, textarea.value))
    }
    // During a composition xterm ignores 229 too, so this only removes the broken fallback.
    term.attachCustomKeyEventHandler((event) => !(event.type === 'keydown' && event.keyCode === 229))
    const listeners = { keydown: onKeyDown, keypress: onKeyPress, beforeinput: onBeforeInput, input: onInput }
    for (const [type, listener] of Object.entries(listeners)) element.addEventListener(type, listener, true)
    this.imeCleanup = () => {
      for (const [type, listener] of Object.entries(listeners)) element.removeEventListener(type, listener, true)
    }
  }

  /**
   * One ack per 64 KiB xterm actually parsed, carrying the absolute end offset of the last
   * parsed chunk. Acks are cheap hints: a failure is ignored, nothing is sent after dispose,
   * and hidden panes keep acking so their stream never stalls.
   */
  private parsedChunk(id: string, seq: number, length: number): void {
    this.parsed += length
    if (this.disposed || this.parsed < ACK_THRESHOLD) return
    this.parsed = 0
    void this.service.ack(id, seq + length).catch(noop)
  }

  /** Writes stay ordered: each one starts after the previous invoke settles. */
  private send(data: string): void {
    if (!data) return
    this.writes = this.writes.then(() => this.service.write(this.sessionId, data)).catch(noop)
  }

  /** Fit, then tell the daemon once per changed size. A hidden (0x0) element is skipped. */
  private async syncSize(): Promise<void> {
    const { term, element } = this
    if (this.disposed || !this.visible || !term || !element?.clientWidth || !element.clientHeight) return
    this.fit.fit()
    const size = `${term.cols}x${term.rows}`
    if (size === this.sent) return
    this.sent = size
    // Keep replay visible after errors and let a later observer event retry this size.
    await this.service.resize(this.sessionId, term.cols, term.rows).catch(() => {
      if (this.sent === size) this.sent = ''
    })
  }
}
