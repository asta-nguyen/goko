import type { GitViewSearchMatch } from '../../services/types'

export type GoToTarget = { kind: 'import'; specifier: string } | { kind: 'name'; name: string }

/** A clickable token; `from`/`to` are columns in its line. */
export interface GoToToken {
  target: GoToTarget
  from: number
  to: number
}

/** A `tsconfig` `paths` entry with its targets already resolved to checkout-relative templates. */
export interface PathAlias {
  pattern: string
  targets: string[]
}

const IMPORT = /\b(?:from|import|require)\s*\(?\s*(['"])(.+?)\1/g
const WORD = /[\p{L}\p{N}_$]+/gu
const KEYWORDS = new Set(
  'const let var function class interface type enum import export from return if else for while new this async await def fn pub use'.split(' '),
)
const EXTENSIONS = ['.ts', '.tsx', '.js', '.jsx', '.mjs', '.cjs', '.json']
// ESM sources import `./x.js` for what is `./x.ts` on disk.
const JS_TO_TS: Record<string, string[]> = { '.js': ['.ts', '.tsx'], '.jsx': ['.tsx'], '.mjs': ['.mts'], '.cjs': ['.cts'] }

/** The import specifier or name under `column` in `line`, if it is worth a lookup. */
export function tokenAt(line: string, column: number): GoToToken | null {
  for (const match of line.matchAll(IMPORT)) {
    const to = (match.index ?? 0) + match[0].length - 1 // the closing quote
    const from = to - match[2].length
    if (column >= from && column <= to) return { target: { kind: 'import', specifier: match[2] }, from, to }
  }
  for (const match of line.matchAll(WORD)) {
    const from = match.index ?? 0
    const to = from + match[0].length
    if (column < from) break
    if (column > to) continue
    const name = match[0]
    return name.length < 2 || /^\d/.test(name) || KEYWORDS.has(name) ? null : { target: { kind: 'name', name }, from, to }
  }
  return null
}

const dirname = (path: string) => path.slice(0, Math.max(0, path.lastIndexOf('/')))

/** `base` + `relative`, normalised; null when `..` climbs out of the checkout. */
function join(base: string, relative: string): string | null {
  const parts = base ? base.split('/') : []
  for (const segment of relative.split('/')) {
    if (segment === '' || segment === '.') continue
    if (segment === '..') {
      if (!parts.length) return null
      parts.pop()
    } else {
      parts.push(segment)
    }
  }
  return parts.join('/')
}

function firstExisting(files: ReadonlySet<string>, base: string): string | null {
  const extension = /\.[^./]+$/.exec(base)?.[0]
  const swapped = extension && JS_TO_TS[extension] ? JS_TO_TS[extension].map((e) => base.slice(0, -extension.length) + e) : []
  const index = base ? `${base}/index` : 'index'
  const candidates = [base, ...swapped, ...EXTENSIONS.map((e) => base + e), ...EXTENSIONS.map((e) => index + e)]
  return candidates.find((candidate) => files.has(candidate)) ?? null
}

/** The checkout file a specifier names: relative paths, then `paths` aliases. Packages resolve to null. */
export function resolveImport(
  files: ReadonlySet<string>,
  fromPath: string,
  specifier: string,
  aliases: readonly PathAlias[] = [],
): string | null {
  if (specifier === '.' || specifier === '..' || specifier.startsWith('./') || specifier.startsWith('../')) {
    const base = join(dirname(fromPath), specifier)
    return base === null ? null : firstExisting(files, base)
  }
  for (const { pattern, targets } of aliases) {
    const star = pattern.indexOf('*')
    let wildcard = ''
    if (star === -1) {
      if (specifier !== pattern) continue
    } else {
      const head = pattern.slice(0, star)
      const tail = pattern.slice(star + 1)
      if (specifier.length < head.length + tail.length || !specifier.startsWith(head) || !specifier.endsWith(tail)) continue
      wildcard = specifier.slice(head.length, specifier.length - tail.length)
    }
    for (const target of targets) {
      const hit = firstExisting(files, target.replace('*', wildcard))
      if (hit) return hit
    }
  }
  return null
}

/** `tsconfig.json`/`jsconfig.json` paths from the file's directory up to the checkout root, nearest first. */
export function configCandidates(fromPath: string): string[] {
  const found: string[] = []
  for (let dir = dirname(fromPath); ; dir = dirname(dir)) {
    for (const name of ['tsconfig.json', 'jsconfig.json']) found.push(dir ? `${dir}/${name}` : name)
    if (!dir) return found
  }
}

const stripJsonc = (text: string) =>
  text
    .replace(/("(?:\\.|[^"\\])*")|\/\/[^\n]*|\/\*[\s\S]*?\*\//g, (_match, string?: string) => string ?? '')
    .replace(/,(\s*[}\]])/g, '$1')

/** `compilerOptions.paths` of one config (`extends` is not followed); unreadable configs give no aliases. */
export function parseTsconfigPaths(text: string, configPath: string): PathAlias[] {
  let options: { baseUrl?: unknown; paths?: unknown } | undefined
  try {
    options = JSON.parse(stripJsonc(text))?.compilerOptions
  } catch {
    return []
  }
  const paths = options?.paths
  if (!paths || typeof paths !== 'object') return []
  const root = join(dirname(configPath), typeof options?.baseUrl === 'string' ? options.baseUrl : '.')
  if (root === null) return []
  return Object.entries(paths).flatMap(([pattern, targets]) => {
    if (!Array.isArray(targets)) return []
    const resolved = targets.flatMap((target) => {
      const full = typeof target === 'string' ? join(root, target) : null
      return full === null ? [] : [full]
    })
    return resolved.length ? [{ pattern, targets: resolved }] : []
  })
}

/** Whether a source line declares `name`: JS/TS, Python, Rust, Go and similar keyword declarations. */
function declares(name: string): (text: string) => boolean {
  const id = `(?<![\\w$])${name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}(?![\\w$])`
  const patterns = [
    `\\b(?:function\\*?|class|interface|type|enum|namespace|struct|trait|fn|func|def|mod|module|object|const|let|var|val|static)\\s+${id}`,
    `\\bfunc\\s*\\([^)]*\\)\\s*${id}`, // Go method
    `^\\s*(?:(?:public|private|protected|static|async|override|abstract|readonly|get|set)\\s+)*${id}\\s*(?:<[^>]*>)?\\([^)]*\\)\\s*(?::\\s*[^{;=]+)?\\{\\s*$`, // method
    `${id}\\s*[:=]\\s*(?:async\\s+)?(?:\\([^)]*\\)|[\\w$]+)\\s*(?::\\s*[^=]+)?=>`, // arrow property
    `${id}\\s*[:=]\\s*(?:async\\s+)?function\\b`,
  ].map((source) => new RegExp(source))
  return (text) => patterns.some((pattern) => pattern.test(text))
}

/**
 * The declaration to jump to among content-search matches for `name`: the open file's own
 * first, else the only one in the checkout. Null (none, or several) means "show the search".
 */
export function pickDefinition(
  matches: readonly GitViewSearchMatch[],
  name: string,
  currentPath: string,
): { path: string; line: number } | null {
  const test = declares(name)
  const found = matches.filter((match) => test(match.text))
  const hit = found.find((match) => match.path === currentPath) ?? (found.length === 1 ? found[0] : undefined)
  return hit ? { path: hit.path, line: hit.line } : null
}
