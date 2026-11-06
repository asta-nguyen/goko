import { shellPath, startFileDrop } from './fileDrop'

const { listen, handlers } = vi.hoisted(() => {
  const handlers = new Map<string, (e: { payload: unknown }) => void>()
  const listen = vi.fn(async (name: string, cb: (e: { payload: unknown }) => void) => {
    handlers.set(name, cb)
    return () => void handlers.delete(name)
  })
  return { listen, handlers }
})
vi.mock('@tauri-apps/api/event', () => ({ listen }))

describe('shellPath', () => {
  it('escapes shell metacharacters and keeps Unicode and plain paths as they are', () => {
    expect(shellPath('/Users/nus/a.png')).toBe('/Users/nus/a.png')
    expect(shellPath('/Users/nus/Screenshot 2026 (1).png')).toBe('/Users/nus/Screenshot\\ 2026\\ \\(1\\).png')
    expect(shellPath("/tmp/it's & $HOME;`x`")).toBe("/tmp/it\\'s\\ \\&\\ \\$HOME\\;\\`x\\`")
    expect(shellPath('/Users/nus/tiếng việt.md')).toBe('/Users/nus/tiếng\\ việt.md')
  })
})

describe('startFileDrop', () => {
  it('hands the escaped, space-joined paths to the callback and ignores an empty drop', async () => {
    const onDrop = vi.fn()
    const stop = startFileDrop(onDrop)
    await vi.waitFor(() => expect(handlers.has('tauri://drag-drop')).toBe(true))

    handlers.get('tauri://drag-drop')!({ payload: { paths: ['/a b.png', '/c.txt'], position: { x: 0, y: 0 } } })
    handlers.get('tauri://drag-drop')!({ payload: { paths: [] } })
    expect(onDrop).toHaveBeenCalledOnce()
    expect(onDrop).toHaveBeenCalledWith('/a\\ b.png /c.txt ')

    stop()
    await vi.waitFor(() => expect(handlers.has('tauri://drag-drop')).toBe(false))
  })
})
