/** One side of a split diff row: the line number in that revision and its text. */
export interface DiffSide {
  no: number
  text: string
}

interface RowBase {
  /** Path of the file section this row belongs to; empty before the first header. */
  file: string
}

export type DiffRow =
  | ({ kind: 'meta'; text: string } & RowBase)
  | ({ kind: 'hunk'; text: string } & RowBase)
  | ({ kind: 'context'; left: DiffSide; right: DiffSide } & RowBase)
  | ({ kind: 'change'; left: DiffSide; right: DiffSide } & RowBase)
  | ({ kind: 'del'; left: DiffSide } & RowBase)
  | ({ kind: 'add'; right: DiffSide } & RowBase)

export interface UnifiedLine {
  kind: 'meta' | 'hunk' | 'context' | 'del' | 'add'
  no: number | null
  prefix: '' | ' ' | '-' | '+'
  text: string
  file: string
}

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/
const FILE_HEADER = /^(diff --git |index |new file mode|deleted file mode|similarity index|dissimilarity index|rename (from|to) |copy (from|to) |Binary files |old mode |new mode )/
const FILE_PATH = /^diff --git a\/.+ b\/(.+)$/

/**
 * Parse a unified diff into aligned two-column rows for the split view.
 *
 * Inside a hunk, a run of deletions directly followed by a run of additions is
 * paired index by index (GitHub-style); the longer run keeps single-sided rows.
 * File and hunk headers, `\ No newline` markers, and anything outside a hunk
 * become full-width meta rows.
 */
export function toSplitRows(patch: string): DiffRow[] {
  const rows: DiffRow[] = []
  const lines = patch.split('\n')
  let oldNo = 0
  let newNo = 0
  let inHunk = false
  let file = ''
  let index = 0

  while (index < lines.length) {
    const line = lines[index]
    if (index === lines.length - 1 && line === '') break

    const path = FILE_PATH.exec(line)
    if (path) {
      file = path[1]
      inHunk = false
      rows.push({ kind: 'meta', text: line, file })
      index++
      continue
    }
    const hunk = HUNK.exec(line)
    if (hunk) {
      oldNo = Number(hunk[1])
      newNo = Number(hunk[2])
      inHunk = true
      rows.push({ kind: 'hunk', text: line, file })
      index++
      continue
    }
    if (FILE_HEADER.test(line)) {
      inHunk = false
      rows.push({ kind: 'meta', text: line, file })
      index++
      continue
    }
    if (!inHunk) {
      rows.push({ kind: 'meta', text: line, file })
      index++
      continue
    }
    if (line.startsWith('\\')) {
      rows.push({ kind: 'meta', text: line, file })
      index++
      continue
    }
    if (line.startsWith('-')) {
      const dels: string[] = []
      while (index < lines.length && lines[index].startsWith('-')) {
        dels.push(lines[index].slice(1))
        index++
      }
      const adds: string[] = []
      while (index < lines.length && lines[index].startsWith('+')) {
        adds.push(lines[index].slice(1))
        index++
      }
      const paired = Math.min(dels.length, adds.length)
      for (let k = 0; k < paired; k++) {
        rows.push({ kind: 'change', left: { no: oldNo + k, text: dels[k] }, right: { no: newNo + k, text: adds[k] }, file })
      }
      for (let k = paired; k < dels.length; k++) rows.push({ kind: 'del', left: { no: oldNo + k, text: dels[k] }, file })
      for (let k = paired; k < adds.length; k++) rows.push({ kind: 'add', right: { no: newNo + k, text: adds[k] }, file })
      oldNo += dels.length
      newNo += adds.length
      continue
    }
    if (line.startsWith('+')) {
      rows.push({ kind: 'add', right: { no: newNo, text: line.slice(1) }, file })
      newNo++
      index++
      continue
    }
    if (line.startsWith(' ')) {
      rows.push({
        kind: 'context',
        left: { no: oldNo, text: line.slice(1) },
        right: { no: newNo, text: line.slice(1) },
        file,
      })
      oldNo++
      newNo++
      index++
      continue
    }
    if (line === '') {
      index++
      continue
    }
    // `--- ` and `+++ ` headers appear before the first hunk; inside a hunk they
    // are deletion/addition lines and were consumed above.
    rows.push({ kind: 'meta', text: line, file })
    index++
  }
  return rows
}

/** Expand parsed rows into the single-column lines the unified view renders. */
export function toUnifiedLines(rows: DiffRow[]): UnifiedLine[] {
  const lines: UnifiedLine[] = []
  for (const row of rows) {
    switch (row.kind) {
      case 'meta':
      case 'hunk':
        lines.push({ kind: row.kind, no: null, prefix: '', text: row.text, file: row.file })
        break
      case 'context':
        lines.push({ kind: 'context', no: row.right.no, prefix: ' ', text: row.right.text, file: row.file })
        break
      case 'change':
        lines.push({ kind: 'del', no: row.left.no, prefix: '-', text: row.left.text, file: row.file })
        lines.push({ kind: 'add', no: row.right.no, prefix: '+', text: row.right.text, file: row.file })
        break
      case 'del':
        lines.push({ kind: 'del', no: row.left.no, prefix: '-', text: row.left.text, file: row.file })
        break
      case 'add':
        lines.push({ kind: 'add', no: row.right.no, prefix: '+', text: row.right.text, file: row.file })
        break
    }
  }
  return lines
}
