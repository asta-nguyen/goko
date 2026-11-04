import { LanguageDescription } from '@codemirror/language'
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
