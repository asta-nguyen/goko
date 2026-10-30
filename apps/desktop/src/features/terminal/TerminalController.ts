import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'
import { toBridgeError } from '../../services/errors'
import type { SessionService } from '../../services/session'

type Service = Pick<SessionService, 'subscribe' | 'write' | 'resize'>

const RESIZE_DEBOUNCE_MS = 100
const noop = () => {}
// Colours come from the theme tokens (CSS variables), read once at mount.
const token = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim() || undefined

/** One xterm instance bound to one session. Output goes straight to `term.write`, never through React. */
export class TerminalController {
  private term?: Terminal
  private element?: HTMLElement
  private fit = new FitAddon()
  private observer?: ResizeObserver
  private timer?: ReturnType<typeof setTimeout>
  private onData?: { dispose(): void }
  private unsubscribe?: () => void
  private sent = ''
  private disposed = false

  constructor(
    private sessionId: string,
    private service: Service,
  ) {}

  /** Open, fit, send the fitted size, then subscribe, so the replay never lands in a wrong-size terminal. */
  async mount(element: HTMLElement): Promise<void> {
    if (this.term || this.disposed) return
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
    // Writes to an exited session are rejected by the daemon; the view's banner shows why.
    this.onData = term.onData((data) => void service.write(id, data).catch(noop))
    this.observer = new ResizeObserver(() => {
      clearTimeout(this.timer)
      this.timer = setTimeout(() => void this.syncSize(), RESIZE_DEBOUNCE_MS)
    })
    this.observer.observe(element)

    await this.syncSize()
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
    this.unsubscribe?.()
    this.onData?.dispose()
    this.fit.dispose()
    this.term?.dispose()
  }

  /** Fit, then tell the daemon once per changed size. A hidden (0x0) element is skipped. */
  private async syncSize(): Promise<void> {
    const { term, element } = this
    if (this.disposed || !term || !element?.clientWidth || !element.clientHeight) return
    this.fit.fit()
    const size = `${term.cols}x${term.rows}`
    if (size === this.sent) return
    this.sent = size
    // A rejected resize (e.g. exited session) must not stop the replay from being shown.
    await this.service.resize(this.sessionId, term.cols, term.rows).catch(noop)
  }
}
