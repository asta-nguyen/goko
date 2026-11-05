import { Annotation, Compartment, EditorState } from '@codemirror/state'
import { LanguageDescription } from '@codemirror/language'
import { EditorView } from '@codemirror/view'
import { basicSetup } from 'codemirror'
import { languages } from '@codemirror/language-data'
import { getCM, Vim, vim } from '@replit/codemirror-vim'
import { useEffect, useRef } from 'react'
import { codeTheme } from './code-theme'

export interface CodeEditorProps {
  value: string
  path: string
  ariaLabel: string
  readOnly: boolean
  vim: boolean
  reveal: { line: number; nonce: number } | null
  /** Called once the cursor has moved, so the caller can drop the request and not replay it on remount. */
  onRevealed?: () => void
  onChange: (text: string) => void
  onSave: () => void
}

/**
 * macOS Telex in WKWebView inserts the typed key from an `input` event that lands before the
 * keydown Vim can preventDefault, so `j` would move *and* type. Placed after `vim()`, this only
 * sees text Vim declined, and outside insert mode no text is ever wanted.
 */
const dropTextOutsideInsertMode = EditorView.inputHandler.of((view) => {
  const state = getCM(view)?.state.vim
  return !!state && !state.insertMode
})
const vimExtensions = () => [vim(), dropTextOutsideInsertMode]

const externalValue = Annotation.define<boolean>()
let saveMountedEditor: (() => void) | null = null
Vim.defineEx('write', 'w', () => saveMountedEditor?.())

export function CodeEditor({ value, path, ariaLabel, readOnly, vim: vimEnabled, reveal, onRevealed, onChange, onSave }: CodeEditorProps) {
  const parent = useRef<HTMLDivElement>(null)
  const view = useRef<EditorView | null>(null)
  const language = useRef<Compartment | null>(null)
  const editable = useRef<Compartment | null>(null)
  const vimMode = useRef<Compartment | null>(null)
  const onChangeRef = useRef(onChange)
  const onSaveRef = useRef(onSave)
  const onRevealedRef = useRef(onRevealed)
  onChangeRef.current = onChange
  onRevealedRef.current = onRevealed
  onSaveRef.current = onSave

  useEffect(() => {
    const parentElement = parent.current
    if (!parentElement) return

    const languageCompartment = new Compartment()
    const editableCompartment = new Compartment()
    const vimCompartment = new Compartment()
    language.current = languageCompartment
    editable.current = editableCompartment
    vimMode.current = vimCompartment

    const editorView = new EditorView({
      doc: value,
      parent: parentElement,
      extensions: [
        vimCompartment.of(vimEnabled ? vimExtensions() : []),
        basicSetup,
        languageCompartment.of([]),
        editableCompartment.of([EditorState.readOnly.of(readOnly), EditorView.editable.of(!readOnly)]),
        EditorView.contentAttributes.of({ 'aria-label': ariaLabel }),
        EditorView.updateListener.of((update) => {
          if (update.docChanged && update.transactions.some((transaction) => !transaction.annotation(externalValue))) {
            onChangeRef.current(update.state.doc.toString())
          }
        }),
        codeTheme,
      ],
    })
    view.current = editorView
    const saveEditor = () => onSaveRef.current()
    saveMountedEditor = saveEditor

    return () => {
      if (saveMountedEditor === saveEditor) saveMountedEditor = null
      editorView.destroy()
      view.current = null
      language.current = null
      editable.current = null
      vimMode.current = null
    }
    // The editor instance is intentionally stable for the lifetime of this component.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    const editorView = view.current
    const compartment = language.current
    if (!editorView || !compartment) return

    let cancelled = false
    const description = LanguageDescription.matchFilename(languages, path)
    if (!description) {
      editorView.dispatch({ effects: compartment.reconfigure([]) })
      return
    }

    void description.load().then(
      (support) => {
        if (!cancelled && view.current === editorView) editorView.dispatch({ effects: compartment.reconfigure(support) })
      },
      () => {
        if (!cancelled && view.current === editorView) editorView.dispatch({ effects: compartment.reconfigure([]) })
      },
    )
    return () => {
      cancelled = true
    }
  }, [path])

  useEffect(() => {
    const editorView = view.current
    const compartment = editable.current
    if (editorView && compartment) {
      editorView.dispatch({
        effects: compartment.reconfigure([EditorState.readOnly.of(readOnly), EditorView.editable.of(!readOnly)]),
      })
    }
  }, [readOnly])

  useEffect(() => {
    const editorView = view.current
    const compartment = vimMode.current
    if (editorView && compartment) editorView.dispatch({ effects: compartment.reconfigure(vimEnabled ? vimExtensions() : []) })
  }, [vimEnabled])

  useEffect(() => {
    const editorView = view.current
    if (!editorView || editorView.state.doc.toString() === value) return
    editorView.dispatch({
      changes: { from: 0, to: editorView.state.doc.length, insert: value },
      annotations: externalValue.of(true),
    })
  }, [value])

  useEffect(() => {
    const editorView = view.current
    if (!editorView || !reveal) return
    const line = editorView.state.doc.line(Math.min(Math.max(1, reveal.line), editorView.state.doc.lines))
    editorView.dispatch({
      selection: { anchor: line.from, head: line.to },
      effects: EditorView.scrollIntoView(line.from, { y: 'center' }),
    })
    editorView.focus()
    onRevealedRef.current?.()
  }, [reveal?.line, reveal?.nonce])

  return <div ref={parent} className="min-h-0 min-w-0 flex-1 overflow-hidden bg-bg" />
}
