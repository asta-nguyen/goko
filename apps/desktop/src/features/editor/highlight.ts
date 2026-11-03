import { useEffect, useMemo, useState } from 'react'
import type { HLJSApi } from 'highlight.js'

/** Beyond this many lines a preview stays plain to keep highlighting cheap. */
const MAX_HIGHLIGHT_LINES = 4000

const LANGUAGE_LOADERS: Record<string, () => Promise<{ default: import('highlight.js').LanguageFn }>> = {
  typescript: () => import('highlight.js/lib/languages/typescript'),
  javascript: () => import('highlight.js/lib/languages/javascript'),
  json: () => import('highlight.js/lib/languages/json'),
  markdown: () => import('highlight.js/lib/languages/markdown'),
  css: () => import('highlight.js/lib/languages/css'),
  xml: () => import('highlight.js/lib/languages/xml'),
  rust: () => import('highlight.js/lib/languages/rust'),
  python: () => import('highlight.js/lib/languages/python'),
  bash: () => import('highlight.js/lib/languages/bash'),
  yaml: () => import('highlight.js/lib/languages/yaml'),
  ini: () => import('highlight.js/lib/languages/ini'),
  sql: () => import('highlight.js/lib/languages/sql'),
  go: () => import('highlight.js/lib/languages/go'),
  diff: () => import('highlight.js/lib/languages/diff'),
}

const EXTENSIONS: Record<string, string> = {
  ts: 'typescript',
  tsx: 'typescript',
  mts: 'typescript',
  cts: 'typescript',
  js: 'javascript',
  jsx: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  json: 'json',
  md: 'markdown',
  mdx: 'markdown',
  css: 'css',
  scss: 'css',
  less: 'css',
  html: 'xml',
  htm: 'xml',
  xml: 'xml',
  svg: 'xml',
  rs: 'rust',
  py: 'python',
  sh: 'bash',
  bash: 'bash',
  zsh: 'bash',
  yml: 'yaml',
  yaml: 'yaml',
  toml: 'ini',
  ini: 'ini',
  sql: 'sql',
  go: 'go',
  diff: 'diff',
  patch: 'diff',
}

/** The highlight.js language for a file path, or null when unsupported. */
export function langForPath(path: string | null | undefined): string | null {
  if (!path) return null
  const name = path.split('/').at(-1) ?? ''
  const dot = name.lastIndexOf('.')
  if (dot < 0) return null
  return EXTENSIONS[name.slice(dot + 1).toLowerCase()] ?? null
}

let core: Promise<HLJSApi> | null = null
const registered = new Set<string>()

async function highlighter(language: string): Promise<HLJSApi | null> {
  const loader = LANGUAGE_LOADERS[language]
  if (!loader) return null
  core ??= import('highlight.js/lib/core').then((module) => module.default)
  const hljs = await core
  if (!registered.has(language)) {
    const languageModule = await loader()
    hljs.registerLanguage(language, languageModule.default)
    registered.add(language)
  }
  return hljs
}

/**
 * Split highlighted HTML into per-line strings, reopening spans that cross a
 * newline so every line stays well-formed on its own.
 */
export function splitHighlightedLines(html: string): string[] {
  const lines: string[] = []
  const open: string[] = []
  for (const raw of html.split('\n')) {
    const reopened = [...open]
    for (const match of raw.matchAll(/<span[^>]*>|<\/span>/g)) {
      if (match[0] === '</span>') open.pop()
      else open.push(match[0])
    }
    lines.push(`${reopened.join('')}${raw}${'</span>'.repeat(open.length)}`)
  }
  return lines
}

async function highlightBlock(text: string, language: string): Promise<string[] | null> {
  const hljs = await highlighter(language)
  return hljs ? splitHighlightedLines(hljs.highlight(text, { language, ignoreIllegals: true }).value) : null
}

async function highlightEachLine(lines: string[], language: string): Promise<string[] | null> {
  const hljs = await highlighter(language)
  return hljs
    ? lines.map((line) => (line === '' ? '' : hljs.highlight(line, { language, ignoreIllegals: true }).value))
    : null
}

/**
 * Highlight each diff line with its own language, for patches whose rows
 * interleave both revisions and may span several files.
 */
export function useHighlightedDiffLines(texts: string[], languages: (string | null)[]) {
  const [html, setHtml] = useState<(string | null)[] | null>(null)
  useEffect(() => {
    let cancelled = false
    setHtml(null)
    if (!languages.some(Boolean) || texts.length > MAX_HIGHLIGHT_LINES) return
    void (async () => {
      const out: (string | null)[] = new Array(texts.length).fill(null)
      for (let index = 0; index < texts.length; index++) {
        const language = languages[index]
        if (!language || texts[index] === '') continue
        const hljs = await highlighter(language)
        if (hljs) out[index] = hljs.highlight(texts[index], { language, ignoreIllegals: true }).value
      }
      if (!cancelled) setHtml(out)
    })().catch(() => {
      if (!cancelled) setHtml(null)
    })
    return () => {
      cancelled = true
    }
  }, [languages, texts])
  return html
}

export type HighlightMode = 'block' | 'lines'

/**
 * Highlighted HTML per line, or null while loading, unsupported, or over the
 * size cap. `block` keeps multi-line constructs correct; `lines` highlights
 * each line alone for diff rows that interleave both revisions.
 */
export function useHighlightedLines(lines: string[], language: string | null, mode: HighlightMode = 'block') {
  const [html, setHtml] = useState<string[] | null>(null)
  const text = useMemo(() => lines.join('\n'), [lines])
  useEffect(() => {
    let cancelled = false
    setHtml(null)
    if (!language || lines.length > MAX_HIGHLIGHT_LINES) return
    const run = mode === 'block' ? highlightBlock(text, language) : highlightEachLine(lines, language)
    run
      .then((result) => {
        if (!cancelled) setHtml(result)
      })
      .catch(() => {
        if (!cancelled) setHtml(null)
      })
    return () => {
      cancelled = true
    }
  }, [language, lines, mode, text])
  return html
}
