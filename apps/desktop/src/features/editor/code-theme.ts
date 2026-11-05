import { HighlightStyle, syntaxHighlighting } from '@codemirror/language'
import type { Extension } from '@codemirror/state'
import { tags } from '@lezer/highlight'
import { EditorView } from '@codemirror/view'

const syntax = HighlightStyle.define([
  { tag: [tags.keyword, tags.definitionKeyword, tags.operatorKeyword], color: 'var(--syntax-keyword)' },
  { tag: tags.controlKeyword, color: 'var(--syntax-control)' },
  { tag: [tags.string, tags.docString, tags.character, tags.regexp, tags.escape], color: 'var(--syntax-string)' },
  { tag: tags.number, color: 'var(--syntax-number)' },
  { tag: [tags.bool, tags.null], color: 'var(--syntax-keyword)' },
  { tag: tags.function(tags.variableName), color: 'var(--syntax-function)' },
  { tag: tags.variableName, color: 'var(--syntax-variable)' },
  { tag: tags.constant(tags.variableName), color: 'var(--syntax-constant)' },
  { tag: [tags.typeName, tags.className, tags.namespace], color: 'var(--syntax-tag)' },
  { tag: [tags.propertyName, tags.attributeName], color: 'var(--syntax-attr)' },
  { tag: [tags.comment, tags.lineComment, tags.blockComment, tags.docComment], color: 'var(--syntax-comment)', fontStyle: 'italic' },
  { tag: [tags.operator, tags.punctuation], color: 'var(--text)' },
])

export const codeTheme: Extension = [
  EditorView.theme({
    '&': { height: '100%', color: 'var(--text)', backgroundColor: 'var(--bg)' },
    '.cm-scroller': { overflow: 'auto', fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace', fontSize: '12px', lineHeight: '1.55' },
    '.cm-content': { padding: '0.5rem 0', caretColor: 'var(--accent)' },
    '.cm-line': { padding: '0 0.75rem' },
    '.cm-gutters': { backgroundColor: 'var(--bg)', color: 'var(--muted)', borderRight: '1px solid var(--border)' },
    '.cm-activeLineGutter': { color: 'var(--text)', backgroundColor: 'color-mix(in srgb, var(--accent) 10%, transparent)' },
    '.cm-activeLine': { backgroundColor: 'color-mix(in srgb, var(--accent) 7%, transparent)' },
    '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--accent)' },
    '.cm-selectionBackground, ::selection': { backgroundColor: 'color-mix(in srgb, var(--accent) 28%, transparent)' },
    '.cm-searchMatch': { backgroundColor: 'color-mix(in srgb, var(--warning) 38%, transparent)' },
    '.cm-searchMatch-selected': { backgroundColor: 'color-mix(in srgb, var(--warning) 62%, transparent)' },
    '.cm-panels': { color: 'var(--text)', backgroundColor: 'var(--panel)' },
    '.cm-panel input': { color: 'var(--text)', backgroundColor: 'var(--bg)', border: '1px solid var(--border)' },
    '.cm-button': { color: 'var(--text)', backgroundColor: 'var(--panel)', border: '1px solid var(--border)' },
    '&.cm-focused': { outline: '2px solid var(--accent)', outlineOffset: '-2px' },
  }),
  syntaxHighlighting(syntax),
]
