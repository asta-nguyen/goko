import { langForPath, splitHighlightedLines } from './highlight'

describe('langForPath', () => {
  it('maps common extensions to highlight.js languages', () => {
    expect(langForPath('src/app.ts')).toBe('typescript')
    expect(langForPath('src/app.tsx')).toBe('typescript')
    expect(langForPath('index.js')).toBe('javascript')
    expect(langForPath('Cargo.toml')).toBe('ini')
    expect(langForPath('README.md')).toBe('markdown')
    expect(langForPath('crates/daemon/src/main.rs')).toBe('rust')
    expect(langForPath('scripts/run.sh')).toBe('bash')
    expect(langForPath('config.yaml')).toBe('yaml')
  })

  it('returns null for unknown or missing paths', () => {
    expect(langForPath('Makefile')).toBeNull()
    expect(langForPath('.env')).toBeNull()
    expect(langForPath('archive.bin')).toBeNull()
    expect(langForPath(null)).toBeNull()
    expect(langForPath(undefined)).toBeNull()
  })
})

describe('splitHighlightedLines', () => {
  it('reopens spans that cross a newline so every line is well-formed', () => {
    const html = '<span class="hljs-comment">a\nb</span>\nplain'
    expect(splitHighlightedLines(html)).toEqual([
      '<span class="hljs-comment">a</span>',
      '<span class="hljs-comment">b</span>',
      'plain',
    ])
  })

  it('keeps nested spans balanced per line', () => {
    const html = '<span class="hljs-string">x<span class="hljs-subst">y\nz</span>w</span>'
    const lines = splitHighlightedLines(html)
    expect(lines).toEqual([
      '<span class="hljs-string">x<span class="hljs-subst">y</span></span>',
      '<span class="hljs-string"><span class="hljs-subst">z</span>w</span>',
    ])
  })

  it('passes through lines without markup', () => {
    expect(splitHighlightedLines('a\nb')).toEqual(['a', 'b'])
  })
})
