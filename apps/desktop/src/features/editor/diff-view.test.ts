import { toSplitRows, toUnifiedLines } from './diff-view'

const PATCH = [
  'diff --git a/a.ts b/a.ts',
  'index 111..222 100644',
  '--- a/a.ts',
  '+++ b/a.ts',
  '@@ -1,3 +1,3 @@',
  ' keep',
  '-old',
  '+new',
  ' tail',
].join('\n')

describe('toSplitRows', () => {
  it('pairs deletions with additions and numbers both sides', () => {
    const rows = toSplitRows(PATCH)
    expect(rows.map((row) => row.kind)).toEqual(['meta', 'meta', 'meta', 'meta', 'hunk', 'context', 'change', 'context'])
    expect(rows[0]).toEqual({ kind: 'meta', text: 'diff --git a/a.ts b/a.ts', file: 'a.ts' })
    expect(rows[5]).toEqual({ kind: 'context', left: { no: 1, text: 'keep' }, right: { no: 1, text: 'keep' }, file: 'a.ts' })
    expect(rows[6]).toEqual({ kind: 'change', left: { no: 2, text: 'old' }, right: { no: 2, text: 'new' }, file: 'a.ts' })
    expect(rows[7]).toEqual({ kind: 'context', left: { no: 3, text: 'tail' }, right: { no: 3, text: 'tail' }, file: 'a.ts' })
  })

  it('keeps unpaired deletions single-sided', () => {
    const rows = toSplitRows(['@@ -5,2 +5,1 @@', '-a', '-b', '+c'].join('\n'))
    expect(rows).toEqual([
      { kind: 'hunk', text: '@@ -5,2 +5,1 @@', file: '' },
      { kind: 'change', left: { no: 5, text: 'a' }, right: { no: 5, text: 'c' }, file: '' },
      { kind: 'del', left: { no: 6, text: 'b' }, file: '' },
    ])
  })

  it('adds without a preceding deletion', () => {
    const rows = toSplitRows(['@@ -1,1 +1,2 @@', ' same', '+added'].join('\n'))
    expect(rows).toEqual([
      { kind: 'hunk', text: '@@ -1,1 +1,2 @@', file: '' },
      { kind: 'context', left: { no: 1, text: 'same' }, right: { no: 1, text: 'same' }, file: '' },
      { kind: 'add', right: { no: 2, text: 'added' }, file: '' },
    ])
  })

  it('treats file headers, binary and no-newline markers as full-width rows', () => {
    const rows = toSplitRows(
      [
        'diff --git a/img.png b/img.png',
        'new file mode 100644',
        'Binary files /dev/null and b/img.png differ',
        '@@ -1,1 +1,1 @@',
        '-gone',
        '\\ No newline at end of file',
      ].join('\n'),
    )
    expect(rows[0]).toEqual({ kind: 'meta', text: 'diff --git a/img.png b/img.png', file: 'img.png' })
    expect(rows[1]).toEqual({ kind: 'meta', text: 'new file mode 100644', file: 'img.png' })
    expect(rows[2]).toEqual({ kind: 'meta', text: 'Binary files /dev/null and b/img.png differ', file: 'img.png' })
    expect(rows[3]).toEqual({ kind: 'hunk', text: '@@ -1,1 +1,1 @@', file: 'img.png' })
    expect(rows[4]).toEqual({ kind: 'del', left: { no: 1, text: 'gone' }, file: 'img.png' })
    expect(rows[5]).toEqual({ kind: 'meta', text: '\\ No newline at end of file', file: 'img.png' })
  })

  it('keeps deletion lines that look like a --- header inside a hunk', () => {
    const rows = toSplitRows(['@@ -1,1 +1,1 @@', '--- old header-like'].join('\n'))
    expect(rows[1]).toEqual({ kind: 'del', left: { no: 1, text: '-- old header-like' }, file: '' })
  })

  it('continues numbering across hunks and handles a truncated patch', () => {
    const rows = toSplitRows(['@@ -10,1 +10,1 @@', '-x', '+y', '@@ -40,1 +41,1 @@', ' z'].join('\n'))
    expect(rows[1]).toEqual({ kind: 'change', left: { no: 10, text: 'x' }, right: { no: 10, text: 'y' }, file: '' })
    expect(rows[2]).toEqual({ kind: 'hunk', text: '@@ -40,1 +41,1 @@', file: '' })
    expect(rows[3]).toEqual({ kind: 'context', left: { no: 40, text: 'z' }, right: { no: 41, text: 'z' }, file: '' })
  })

  it('tracks the file of each section so mixed patches keep their languages', () => {
    const rows = toSplitRows(
      [
        'diff --git a/src/a.ts b/src/a.ts',
        '@@ -1,1 +1,1 @@',
        '-a',
        '+b',
        'diff --git a/README.md b/README.md',
        '@@ -1,1 +1,1 @@',
        '-c',
        '+d',
      ].join('\n'),
    )
    expect(rows.find((row) => row.kind === 'change')).toMatchObject({ file: 'src/a.ts' })
    expect(rows.filter((row) => row.kind === 'change').at(-1)).toMatchObject({ file: 'README.md' })
  })
})

describe('toUnifiedLines', () => {
  it('expands paired rows into deletion and addition lines with numbers', () => {
    const lines = toUnifiedLines(toSplitRows(PATCH))
    expect(lines).toEqual([
      { kind: 'meta', no: null, prefix: '', text: 'diff --git a/a.ts b/a.ts', file: 'a.ts' },
      { kind: 'meta', no: null, prefix: '', text: 'index 111..222 100644', file: 'a.ts' },
      { kind: 'meta', no: null, prefix: '', text: '--- a/a.ts', file: 'a.ts' },
      { kind: 'meta', no: null, prefix: '', text: '+++ b/a.ts', file: 'a.ts' },
      { kind: 'hunk', no: null, prefix: '', text: '@@ -1,3 +1,3 @@', file: 'a.ts' },
      { kind: 'context', no: 1, prefix: ' ', text: 'keep', file: 'a.ts' },
      { kind: 'del', no: 2, prefix: '-', text: 'old', file: 'a.ts' },
      { kind: 'add', no: 2, prefix: '+', text: 'new', file: 'a.ts' },
      { kind: 'context', no: 3, prefix: ' ', text: 'tail', file: 'a.ts' },
    ])
  })
})
