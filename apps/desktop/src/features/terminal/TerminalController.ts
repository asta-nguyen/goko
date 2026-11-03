import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'
import { toBridgeError } from '../../services/errors'
import type { SessionService } from '../../services/session'

type Service = Pick<SessionService, 'subscribe' | 'write' | 'resize'>

const RESIZE_DEBOUNCE_MS = 100
// WKWebView reports IME keys with keyCode 229; the committed text follows within this window.
const IME_KEY_WINDOW_MS = 100
const BACKSPACE = '\x7f'
const noop = () => {}
// Colours come from the theme tokens (CSS variables), read once at mount.
const token = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim() || undefined

const commonPrefix = (left: string, right: string): number => {
  const max = Math.min(left.length, right.length)
  let index = 0
  while (index < max && left[index] === right[index]) index += 1
  return index
}

/** One xterm instance bound to one session. Output goes straight to `term.write`, never through React. */
export class TerminalController {
  private term?: Terminal
  private element?: HTMLElement
  private fit = new FitAddon()
  private observer?: ResizeObserver
  private timer?: ReturnType<typeof setTimeout>
  private onData?: { dispose(): void }
  private imeCleanup?: () => void
  private unsubscribe?: () => void
  private sent = ''
  private disposed = false
  private visible = true
  /** Model of what the terminal already has for the IME line being typed. */
  private line = ''
  private imeKeyAt = 0
  /** True once this IME has proved it commits whole values instead of composition events. */
  private replacementIme = false
  private writes: Promise<void> = Promise.resolve()

  constructor(
    private sessionId: string,
    private service: Service,
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
    term.loadAddon(this.fit)
    term.open(element)
    this.attachIme(term)
    // Writes to an exited session are rejected by the daemon; the view's banner shows why.
    this.onData = term.onData((data) => this.onTerminalData(data))
    this.observer = new ResizeObserver(() => {
      clearTimeout(this.timer)
      this.timer = setTimeout(() => void this.syncSize(), RESIZE_DEBOUNCE_MS)
    })
    this.observer.observe(element)

    if (this.visible) await this.syncSize()
    if (this.disposed) return
    try {
      const unsubscribe = await service.subscribe(id, (_seq, data) => {
        if (!this.disposed) term.write(data)
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
    clearTimeout(this.timer)
    this.observer?.disconnect()
    this.imeCleanup?.()
    this.unsubscribe?.()
    this.onData?.dispose()
    this.fit.dispose()
    this.term?.dispose()
  }

  setVisible(visible: boolean): void {
    if (this.disposed || this.visible === visible) return
    this.visible = visible
    if (visible) void this.syncSize()
  }

  /**
   * WKWebView does not fire composition events for every IME: macOS Simple Telex and 2-Set Korean
   * send each key as keyCode 229 plus an `input` event that replaces the whole textarea value.
   * xterm only forwards the first diff, so the word is lost. Forward the delta ourselves and drop
   * xterm's duplicate diff until the key that ends the word.
   */
  private attachIme(term: Terminal): void {
    const textarea = term.textarea
    if (!textarea) return
    const onInput = (event: Event) => {
      const input = event as InputEvent
      if (input.isComposing) return
      const replacement = input.inputType === 'insertReplacementText'
      const first = input.inputType === 'insertText' && Date.now() - this.imeKeyAt < IME_KEY_WINDOW_MS
      if (!replacement && !first) return
      if (replacement) this.replacementIme = true
      this.acceptIme(textarea)
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.target !== textarea) return
      if (event.keyCode === 229) {
        this.imeKeyAt = Date.now()
        return
      }
      // Only keys that end or rewrite the line invalidate the model; WKWebView also repeats a
      // printable key with its real keyCode, and that one is still part of the IME text.
      if (
        event.key === 'Enter' ||
        event.key === 'Tab' ||
        event.key === 'Escape' ||
        (event.ctrlKey && event.key.toLowerCase() === 'c')
      ) {
        this.resetLine()
      }
    }
    const onBlur = () => this.resetLine()
    // xterm's own keyCode-229 fallback appends partial text for this IME; the handler above
    // sends the committed value instead, so keep xterm out of the way once that shape is known.
    term.attachCustomKeyEventHandler(
      (event) => !(event.type === 'keydown' && event.keyCode === 229 && this.replacementIme),
    )
    textarea.addEventListener('input', onInput)
    textarea.addEventListener('blur', onBlur)
    document.addEventListener('keydown', onKeyDown, true)
    this.imeCleanup = () => {
      textarea.removeEventListener('input', onInput)
      textarea.removeEventListener('blur', onBlur)
      document.removeEventListener('keydown', onKeyDown, true)
    }
  }

  /** Every byte xterm forwards is real input, so keep the line model in step with it. */
  private onTerminalData(data: string): void {
    for (const char of data) {
      if (char === BACKSPACE) this.line = this.line.slice(0, -1)
      else if (char >= ' ') this.line += char // control keys never belong to the IME line
    }
    this.send(data)
  }

  /**
   * WKWebView commits this IME as whole textarea values, and the textarea holds the whole line
   * (`"tiếng vi"`), not the word. Make the terminal match it by sending only the difference.
   */
  private acceptIme(textarea: HTMLTextAreaElement): void {
    // The textarea holds the whole line (`"tiếng vi"`), not the word, and WKWebView writes a
    // non-breaking space for the space key while xterm forwards a real one.
    const next = textarea.value.normalize('NFC').replace(/\u00a0/g, ' ')
    const current = this.line.normalize('NFC')
    const common = commonPrefix(current, next)
    // One chunk per commit keeps the write queue short and the order obvious.
    const chunk = BACKSPACE.repeat(current.length - common) + next.slice(common)
    this.send(chunk)
    this.line = next
  }

  private resetLine(): void {
    this.line = ''
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
