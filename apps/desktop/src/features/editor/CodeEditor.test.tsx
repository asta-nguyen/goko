import { LanguageDescription, syntaxTree } from '@codemirror/language'
import { languages } from '@codemirror/language-data'
import { EditorView } from '@codemirror/view'
import { getCM, Vim, vim } from '@replit/codemirror-vim'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { CodeEditor } from './CodeEditor'

const baseProps = {
  value: 'a\nb',
  path: 'notes.txt',
  ariaLabel: 'Edit notes.txt',
  readOnly: false,
  vim: false,
  reveal: null,
  onChange: vi.fn(),
  onSave: vi.fn(),
}

beforeEach(() => {
  baseProps.onChange.mockReset()
  baseProps.onSave.mockReset()
})

describe('CodeEditor', () => {
  it('mounts the document with line numbers', async () => {
    const { container } = render(<CodeEditor {...baseProps} />)

    await waitFor(() => expect(container.querySelectorAll('.cm-line')).toHaveLength(2))
    expect(container.querySelector('.cm-gutter.cm-lineNumbers')).toBeTruthy()
    expect(screen.getByRole('textbox', { name: 'Edit notes.txt' })).toBeTruthy()
  })

  it('reports edits from the editor', async () => {
    const { container } = render(<CodeEditor {...baseProps} />)
    const content = container.querySelector('.cm-content') as HTMLElement
    content.textContent = 'changed'
    fireEvent.input(content, { inputType: 'insertText', data: 'changed' })

    await waitFor(() => expect(baseProps.onChange).toHaveBeenCalledWith('changed'))
  })

  it('mirrors external text without reporting it as an edit', async () => {
    const { container, rerender } = render(<CodeEditor {...baseProps} />)
    rerender(<CodeEditor {...baseProps} value="reloaded" />)

    await waitFor(() => expect(container.querySelector('.cm-content')?.textContent).toBe('reloaded'))
    expect(baseProps.onChange).not.toHaveBeenCalled()
  })

  it('uses the read-only facet to disable editing', async () => {
    const { container } = render(<CodeEditor {...baseProps} readOnly />)

    await waitFor(() => expect(container.querySelector('.cm-content')?.getAttribute('contenteditable')).toBe('false'))
  })

  it('resolves languages by filename and leaves unknown extensions plain', () => {
    expect(LanguageDescription.matchFilename(languages, 'x.ts')?.name).toBe('TypeScript')
    expect(LanguageDescription.matchFilename(languages, 'x.unknownext')).toBeNull()
    expect(() => render(<CodeEditor {...baseProps} path="x.unknownext" />)).not.toThrow()
  })

  it('runs the mounted save callback for the Vim :w command', async () => {
    const onSave = vi.fn()
    const { container } = render(<CodeEditor {...baseProps} vim onSave={onSave} />)
    await waitFor(() => expect(container.querySelector('.cm-vimMode')).toBeTruthy())
    const commandView = new EditorView({ parent: document.body, extensions: [vim()] })
    Vim.handleEx(getCM(commandView)! as Parameters<typeof Vim.handleEx>[0], 'w')
    commandView.destroy()

    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(1))
  })

  it('swallows typed text in Vim normal mode but not in insert mode', async () => {
    const { container } = render(<CodeEditor {...baseProps} vim />)
    await waitFor(() => expect(container.querySelector('.cm-vimMode')).toBeTruthy())
    const view = EditorView.findFromDOM(container.querySelector('.cm-editor') as HTMLElement)!
    const insertText = () => view.state.facet(EditorView.inputHandler).some((handler) => handler(view, 0, 0, 'j', () => view.state.update({})))

    expect(insertText()).toBe(true)
    Vim.handleKey(getCM(view)! as Parameters<typeof Vim.handleKey>[0], 'i', 'user')
    expect(insertText()).toBe(false)
  })

  it('reports a reveal once and does not repeat it on re-render', async () => {
    const onRevealed = vi.fn()
    const reveal = { line: 2, nonce: 1 }
    const { rerender } = render(
      <CodeEditor {...baseProps} value={'first\nsecond'} reveal={reveal} onRevealed={onRevealed} />,
    )
    await waitFor(() => expect(onRevealed).toHaveBeenCalledTimes(1))

    rerender(<CodeEditor {...baseProps} value={'first\nsecond'} reveal={reveal} onRevealed={onRevealed} />)
    expect(onRevealed).toHaveBeenCalledTimes(1)
  })

  it('selects the clamped reveal line and reselects it for a new nonce', async () => {
    const { container, rerender } = render(
      <CodeEditor {...baseProps} value={'first\nsecond\nlast'} reveal={{ line: 99, nonce: 1 }} />,
    )
    await waitFor(() => expect(document.getSelection()?.toString()).toBe('last'))

    const content = container.querySelector('.cm-content') as HTMLElement
    fireEvent.keyDown(content, { key: 'ArrowLeft' })
    await waitFor(() => expect(document.getSelection()?.toString()).not.toBe('last'))
    rerender(<CodeEditor {...baseProps} value={'first\nsecond\nlast'} reveal={{ line: 99, nonce: 2 }} />)
    await waitFor(() => expect(document.getSelection()?.toString()).toBe('last'))
  })
})

describe('CodeEditor go to', () => {
  const source = "import { x } from './util'\nconst y = sumAll(x) // sumAll"
  const onGoTo = vi.fn()
  const mount = (props: Partial<Parameters<typeof CodeEditor>[0]> = {}) => {
    const { container } = render(<CodeEditor {...baseProps} value={source} path="a.ts" onGoTo={onGoTo} {...props} />)
    const content = container.querySelector('.cm-content') as HTMLElement
    const view = EditorView.findFromDOM(container.querySelector('.cm-editor') as HTMLElement)!
    const at = (needle: string, nth = 0, offset = 1) => {
      let from = -1
      for (let i = 0; i <= nth; i += 1) from = source.indexOf(needle, from + 1)
      vi.spyOn(view, 'posAtCoords').mockReturnValue(from + offset)
    }
    return { container, content, at, view }
  }
  beforeEach(() => onGoTo.mockReset())
  afterEach(() => vi.restoreAllMocks())

  it('reports an import specifier and a name on Ctrl+click, and consumes the click', async () => {
    const { content, at } = mount()
    await waitFor(() => expect(content.querySelectorAll('.cm-line')).toHaveLength(2))

    at("'./util'", 0, 4)
    expect(fireEvent.mouseDown(content, { button: 0, ctrlKey: true })).toBe(false) // preventDefault was called
    expect(onGoTo).toHaveBeenLastCalledWith({ kind: 'import', specifier: './util' })

    at('sumAll', 0, 2)
    fireEvent.mouseDown(content, { button: 0, ctrlKey: true })
    expect(onGoTo).toHaveBeenLastCalledWith({ kind: 'name', name: 'sumAll' })
    expect(onGoTo).toHaveBeenCalledTimes(2)
  })

  it('ignores plain clicks, other buttons, one-letter names, blank space and comments', async () => {
    const { content, at } = mount({ path: 'a.unknownext' })
    await waitFor(() => expect(content.querySelectorAll('.cm-line')).toHaveLength(2))

    at('sumAll', 0, 2)
    fireEvent.mouseDown(content, { button: 0 })
    fireEvent.mouseDown(content, { button: 0, metaKey: true }) // Cmd is not the modifier off macOS
    fireEvent.mouseDown(content, { button: 2, ctrlKey: true })
    at('(x)', 0, 1)
    fireEvent.mouseDown(content, { button: 0, ctrlKey: true })
    at(' //', 0, 0)
    fireEvent.mouseDown(content, { button: 0, ctrlKey: true })
    expect(onGoTo).not.toHaveBeenCalled()
  })

  it('skips names inside comments and strings once the language is loaded, but not import strings', async () => {
    const { content, view } = mount({ value: "import a from './util'\nconst s = 'sumAll' // sumAll\nsumAll(a)" })
    await waitFor(() => expect(syntaxTree(view.state).topNode.name).toBe('Script'))
    const doc = view.state.doc.toString()
    const at2 = (index: number) => vi.spyOn(view, 'posAtCoords').mockReturnValue(index + 2)

    at2(doc.indexOf("'sumAll'") + 1) // inside the string
    fireEvent.mouseDown(content, { button: 0, ctrlKey: true })
    at2(doc.indexOf('// sumAll') + 3) // inside the comment
    fireEvent.mouseDown(content, { button: 0, ctrlKey: true })
    expect(onGoTo).not.toHaveBeenCalled()

    at2(doc.lastIndexOf('sumAll'))
    fireEvent.mouseDown(content, { button: 0, ctrlKey: true })
    at2(doc.indexOf('./util'))
    fireEvent.mouseDown(content, { button: 0, ctrlKey: true })
    expect(onGoTo.mock.calls).toEqual([[{ kind: 'name', name: 'sumAll' }], [{ kind: 'import', specifier: './util' }]])
  })

  it('does nothing without a handler', async () => {
    const { content, at } = mount({ onGoTo: undefined })
    await waitFor(() => expect(content.querySelectorAll('.cm-line')).toHaveLength(2))
    at('sumAll', 0, 2)
    expect(() => fireEvent.mouseDown(content, { button: 0, ctrlKey: true })).not.toThrow()
  })

  it('uses Cmd, not Ctrl, on macOS', async () => {
    vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue('Mozilla/5.0 (Macintosh; Intel Mac OS X)')
    const { content, at } = mount()
    await waitFor(() => expect(content.querySelectorAll('.cm-line')).toHaveLength(2))
    at('sumAll', 0, 2)

    fireEvent.mouseDown(content, { button: 0, ctrlKey: true })
    expect(onGoTo).not.toHaveBeenCalled()
    fireEvent.mouseDown(content, { button: 0, metaKey: true })
    expect(onGoTo).toHaveBeenCalledWith({ kind: 'name', name: 'sumAll' })
  })

  it('underlines the token under a held modifier and clears it on leave', async () => {
    const { container, content, at } = mount()
    await waitFor(() => expect(content.querySelectorAll('.cm-line')).toHaveLength(2))
    at('sumAll', 0, 2)

    fireEvent.mouseMove(content, { ctrlKey: true })
    await waitFor(() => expect(container.querySelector('.cm-goto')?.textContent).toBe('sumAll'))
    fireEvent.mouseMove(content) // modifier released
    await waitFor(() => expect(container.querySelector('.cm-goto')).toBeNull())

    fireEvent.mouseMove(content, { ctrlKey: true })
    await waitFor(() => expect(container.querySelector('.cm-goto')).not.toBeNull())
    fireEvent.mouseLeave(content)
    await waitFor(() => expect(container.querySelector('.cm-goto')).toBeNull())
  })

  it('handles the click ahead of Vim', async () => {
    const { content, at } = mount({ vim: true })
    await waitFor(() => expect(content.querySelectorAll('.cm-line')).toHaveLength(2))
    at('sumAll', 0, 2)

    fireEvent.mouseDown(content, { button: 0, ctrlKey: true })
    expect(onGoTo).toHaveBeenCalledWith({ kind: 'name', name: 'sumAll' })
  })
})
