import { sessionService } from './session'

const { invoke, channels } = vi.hoisted(() => ({
  invoke: vi.fn(),
  channels: [] as { onmessage: (b: ArrayBuffer) => void }[],
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke,
  Channel: class {
    onmessage: (b: ArrayBuffer) => void
    constructor(onmessage: (b: ArrayBuffer) => void) {
      this.onmessage = onmessage
      channels.push(this)
    }
  },
}))

beforeEach(() => {
  invoke.mockReset().mockResolvedValue(undefined)
  channels.length = 0
})

describe('sessionService commands', () => {
  it('calls each command with its payload', async () => {
    await sessionService.create({ command: 'sh -l', cwd: '/tmp', cols: 80, rows: 24, title: 't' })
    await sessionService.create({ agent: 'claude', projectId: 'p1', cols: 80, rows: 24 })
    await sessionService.create({ agent: 'shell', projectId: 'p1', checkout: '/worktree', cols: 80, rows: 24 })
    await sessionService.stop('a', true)
    await sessionService.delete('a')
    await sessionService.list()
    await sessionService.meta()
    await sessionService.write('a', 'ls\r')
    await sessionService.resize('a', 100, 30)
    await sessionService.ack('a', 4096)
    await sessionService.status()
    expect(invoke.mock.calls).toEqual([
      ['session_create', { command: 'sh -l', cwd: '/tmp', cols: 80, rows: 24, title: 't' }],
      ['session_create', { agent: 'claude', projectId: 'p1', cols: 80, rows: 24 }],
      ['session_create', { agent: 'shell', projectId: 'p1', checkout: '/worktree', cols: 80, rows: 24 }],
      ['session_stop', { id: 'a', force: true }],
      ['session_delete', { id: 'a' }],
      ['session_list', undefined],
      ['session_meta_list', undefined],
      ['terminal_write', { id: 'a', data: 'ls\r' }],
      ['terminal_resize', { id: 'a', cols: 100, rows: 30 }],
      ['terminal_ack', { id: 'a', uptoSeq: 4096 }],
      ['daemon_status', undefined],
    ])
  })

  it('turns a rejected invoke into a BridgeError', async () => {
    invoke.mockRejectedValueOnce({ code: 'auth_failed', message: 'nope', hint: null })
    await expect(sessionService.list()).rejects.toEqual({ code: 'auth_failed', message: 'nope', hint: null })
    invoke.mockRejectedValueOnce('plain string')
    await expect(sessionService.list()).rejects.toMatchObject({ code: 'unknown', message: 'plain string' })
    invoke.mockRejectedValueOnce(undefined)
    await expect(sessionService.list()).rejects.toMatchObject({ code: 'unknown' })
  })
})

describe('sessionService.subscribe', () => {
  const frame = (seq: number, bytes: number[]) => {
    const buf = new ArrayBuffer(8 + bytes.length)
    new DataView(buf).setBigUint64(0, BigInt(seq))
    new Uint8Array(buf, 8).set(bytes)
    return buf
  }

  it('passes the channel, decodes seq and bytes, and unsubscribes once', async () => {
    const got: [number, number[]][] = []
    const unsub = await sessionService.subscribe('a', (seq, data) => got.push([seq, [...data]]))

    expect(invoke).toHaveBeenCalledWith('terminal_subscribe', { id: 'a', onOutput: channels[0] })
    channels[0].onmessage(frame(5, [104, 105]))
    channels[0].onmessage(frame(7, []))
    channels[0].onmessage(new ArrayBuffer(3)) // shorter than the seq prefix: ignored
    expect(got).toEqual([[5, [104, 105]], [7, []]])

    unsub()
    unsub()
    const unsubscribes = invoke.mock.calls.filter(([cmd]) => cmd === 'terminal_unsubscribe')
    expect(unsubscribes).toEqual([['terminal_unsubscribe', { id: 'a' }]])
  })

  it('rejects with a BridgeError when subscribing fails', async () => {
    invoke.mockRejectedValueOnce({ code: 'not_found', message: 'no session', hint: null })
    await expect(sessionService.subscribe('x', () => {})).rejects.toMatchObject({ code: 'not_found' })
  })
})
