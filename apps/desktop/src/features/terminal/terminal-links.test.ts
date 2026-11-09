import { findFileRefs, findUrlRefs, resolveFileRef } from './terminal-links'

describe('findUrlRefs', () => {
  it('finds an HTTP(S) URL and no file ref for a URL that looks like a path', () => {
    const text = 'https://example.com/a.ts'
    expect(findUrlRefs(text)).toEqual([{ start: 0, end: text.length, url: text }])
    expect(findFileRefs(text)).toEqual([])
  })

  it('excludes trailing sentence punctuation', () => {
    const url = 'https://example.com/a?q=1'
    expect(findUrlRefs(`see ${url},`)).toEqual([{ start: 4, end: 4 + url.length, url }])
    expect(findUrlRefs('see https://example.com/a.')).toEqual([
      { start: 4, end: 4 + 'https://example.com/a'.length, url: 'https://example.com/a' },
    ])
  })

  it('ignores non-HTTP(S) schemes', () => {
    expect(findUrlRefs('file:///tmp/a.ts')).toEqual([])
    expect(findUrlRefs('javascript:alert(1)')).toEqual([])
  })
})

describe('findFileRefs', () => {
  it('parses :line and :line:col suffixes into the ref', () => {
    expect(findFileRefs('src/App.tsx:10')).toEqual([{ start: 0, end: 14, path: 'src/App.tsx', line: 10 }])
    expect(findFileRefs('a/b.ts:3:7')).toEqual([{ start: 0, end: 10, path: 'a/b.ts', line: 3, col: 7 }])
  })

  it('strips surrounding brackets and trailing punctuation', () => {
    expect(findFileRefs('see (src/a.ts).')).toEqual([{ start: 5, end: 13, path: 'src/a.ts' }])
    expect(findFileRefs('[x](docs/y.md)')).toEqual([{ start: 4, end: 13, path: 'docs/y.md' }])
  })

  it('accepts files with an extension or a slash', () => {
    expect(findFileRefs('README.md').map((ref) => ref.path)).toEqual(['README.md'])
    expect(findFileRefs('./x/y.rs').map((ref) => ref.path)).toEqual(['./x/y.rs'])
  })

  it('rejects bare words and numbers', () => {
    expect(findFileRefs('hello 1.5')).toEqual([])
  })
})

describe('resolveFileRef', () => {
  const checkoutPath = '/work/p1'
  const files = new Set(['src/a.ts', 'src/b.ts', 'docs/y.md', 'a.ts', 'README.md'])
  const resolve = (path: string, cwd: string | undefined = undefined, list = files) =>
    resolveFileRef({ path }, { checkoutPath, cwd, files: list })

  it('maps an absolute path inside the checkout to a relative one', () => {
    expect(resolve('/work/p1/src/a.ts')).toBe('src/a.ts')
  })

  it('drops absolute paths outside the checkout', () => {
    expect(resolve('/etc/passwd')).toBeNull()
  })

  it('drops paths that climb out of the checkout', () => {
    expect(resolve('../../outside.ts', '/work/p1/src')).toBeNull()
  })

  it('resolves a relative path against the session cwd', () => {
    expect(resolve('a.ts', '/work/p1/src')).toBe('src/a.ts')
  })

  it('prefers the cwd match over the same file at the checkout root', () => {
    expect(resolve('a.ts', '/work/p1/src', new Set(['src/a.ts', 'a.ts']))).toBe('src/a.ts')
  })

  it('normalizes . and .. in the cwd before resolving', () => {
    expect(resolve('a.ts', '/work/p1/src/sub/..', new Set(['src/a.ts', 'a.ts']))).toBe('src/a.ts')
  })

  it('falls back to the checkout root when the cwd has no match', () => {
    expect(resolve('a.ts', '/work/p1/src', new Set(['a.ts']))).toBe('a.ts')
  })

  it('returns null for a path absent from the file list', () => {
    expect(resolve('missing.ts', '/work/p1/src')).toBeNull()
  })

  it('falls back to the checkout root when cwd is unknown', () => {
    expect(resolve('a.ts', undefined)).toBe('a.ts')
  })
})
