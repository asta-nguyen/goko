import { Annotation, Compartment, EditorState, Prec, StateEffect, StateField } from '@codemirror/state'
import { LanguageDescription, syntaxTree } from '@codemirror/language'
import { Decoration, EditorView, type DecorationSet } from '@codemirror/view'
import { basicSetup } from 'codemirror'
import { languages } from '@codemirror/language-data'
import { getCM, Vim, vim } from '@replit/codemirror-vim'
import { useEffect, useRef } from 'react'
import { goToModifier } from '../../lib/utils'
import { codeTheme } from './code-theme'
import { tokenAt, type GoToTarget, type GoToToken } from './go-to-definition'

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
  /** Cmd+click (macOS) or Ctrl+click on an import specifier or a name. */
  onGoTo?: (target: GoToTarget) => void
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

/** The token under the pointer; a name inside a comment or string is not worth a lookup. */
function tokenUnder(view: EditorView, event: MouseEvent): GoToToken | null {
  const pos = view.posAtCoords({ x: event.clientX, y: event.clientY })
  if (pos === null) return null
  const line = view.state.doc.lineAt(pos)
  const token = tokenAt(line.text, pos - line.from)
  if (!token) return null
  if (token.target.kind === 'name' && /Comment|String/.test(syntaxTree(view.state).resolveInner(pos, -1).name)) return null
  // A click in the blank space after a line still maps to its last position; it is not on the token.
  const end = view.coordsAtPos(line.from + token.to, -1)
  if (end && event.clientX > end.right + 1) return null
  return { ...token, from: line.from + token.from, to: line.from + token.to }
}

const setHover = StateEffect.define<{ from: number; to: number } | null>()
const goToMark = Decoration.mark({ class: 'cm-goto' })
const hoverField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(decorations, transaction) {
    for (const effect of transaction.effects) {
      if (effect.is(setHover)) return effect.value ? Decoration.set([goToMark.range(effect.value.from, effect.value.to)]) : Decoration.none
    }
    return transaction.docChanged ? Decoration.none : decorations
  },
  provide: (field) => EditorView.decorations.from(field),
})
const goToTheme = EditorView.baseTheme({ '.cm-goto': { textDecoration: 'underline', cursor: 'pointer' } })

/** Placed ahead of `vim()` so the modified click never starts a Vim selection. */
const goToExtensions = (handler: () => ((target: GoToTarget) => void) | undefined) => {
  let hovered = ''
  const hover = (view: EditorView, token: GoToToken | null) => {
    const key = token ? `${token.from}-${token.to}` : ''
    if (key === hovered) return
    hovered = key
    view.dispatch({ effects: setHover.of(token && { from: token.from, to: token.to }) })
  }
  return [
    hoverField,
    goToTheme,
    Prec.highest(
      EditorView.domEventHandlers({
        mousedown(event, view) {
          const onGoTo = handler()
          const token = onGoTo && event.button === 0 && goToModifier(event) ? tokenUnder(view, event) : null
          if (!token || !onGoTo) return false
          event.preventDefault()
          onGoTo(token.target)
          return true
        },
        mousemove(event, view) {
          hover(view, handler() && goToModifier(event) ? tokenUnder(view, event) : null)
        },
        mouseleave(_event, view) {
          hover(view, null)
        },
      }),
    ),
  ]
}

const externalValue = Annotation.define<boolean>()
let saveMountedEditor: (() => void) | null = null
Vim.defineEx('write', 'w', () => saveMountedEditor?.())

export function CodeEditor({ value, path, ariaLabel, readOnly, vim: vimEnabled, reveal, onRevealed, onChange, onSave, onGoTo }: CodeEditorProps) {
  const parent = useRef<HTMLDivElement>(null)
  const view = useRef<EditorView | null>(null)
  const language = useRef<Compartment | null>(null)
  const editable = useRef<Compartment | null>(null)
  const vimMode = useRef<Compartment | null>(null)
  const onChangeRef = useRef(onChange)
  const onSaveRef = useRef(onSave)
  const onRevealedRef = useRef(onRevealed)
  const onGoToRef = useRef(onGoTo)
  onChangeRef.current = onChange
  onRevealedRef.current = onRevealed
  onSaveRef.current = onSave
  onGoToRef.current = onGoTo

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
        goToExtensions(() => onGoToRef.current),
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
