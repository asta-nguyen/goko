export interface UrlRef {
  start: number
  end: number
  url: string
}

export interface FileRef {
  start: number
  end: number
  path: string
  line?: number
  col?: number
}

const URL_PATTERN = /https?:\/\/[^\s<>"'`]+/g
const URL_TRAILING = /[.,;:!?]+$/
const FILE_TOKEN = /[^\s()\[\]{}<>"'`,;]+/g
const TRAILING_DOT_COLON = /[.:]+$/
const LINE_SUFFIX = /^(.*?)(?::(\d+)(?::(\d+))?)?$/
const EXTENSION = /\.[A-Za-z0-9]{1,10}$/

const trimTrailingSlashes = (path: string) => (path.length > 1 ? path.replace(/\/+$/, '') : path)

/** `rel` joined onto checkout-relative `base` with `.`/`..` resolved; null when it climbs out. */
const joinRelative = (base: string, rel: string): string | null => {
  const parts = base ? base.split('/') : []
  for (const segment of rel.split('/')) {
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

/** HTTP(S) URLs in `text`; `start`/`end` are row offsets with trailing sentence punctuation excluded. */
export function findUrlRefs(text: string): UrlRef[] {
  const refs: UrlRef[] = []
  for (const match of text.matchAll(URL_PATTERN)) {
    const url = match[0].replace(URL_TRAILING, '')
    if (!url.slice(url.indexOf('//') + 2)) continue
    const start = match.index ?? 0
    refs.push({ start, end: start + url.length, url })
  }
  return refs
}

/**
 * Path-like tokens in `text`, skipping spans already claimed by a URL. `start`/`end` cover the
 * whole token including `:line:col`; `path` excludes it.
 */
export function findFileRefs(text: string): FileRef[] {
  const urls = findUrlRefs(text)
  const refs: FileRef[] = []
  for (const match of text.matchAll(FILE_TOKEN)) {
    const raw = match[0]
    const start = match.index ?? 0
    const tokenEnd = start + raw.length
    if (urls.some((url) => start < url.end && url.start < tokenEnd)) continue
    const token = raw.replace(TRAILING_DOT_COLON, '')
    if (!token) continue
    const parsed = LINE_SUFFIX.exec(token)!
    const path = parsed[1].replace(TRAILING_DOT_COLON, '')
    const extension = EXTENSION.exec(path)?.[0].slice(1)
    const hasExtension = extension !== undefined && /[A-Za-z]/.test(extension)
    if (!path || (!path.includes('/') && !hasExtension)) continue
    const line = parsed[2] ? Number(parsed[2]) : undefined
    const col = parsed[3] ? Number(parsed[3]) : undefined
    refs.push({
      start,
      end: start + token.length,
      path,
      ...(line === undefined ? {} : { line }),
      ...(col === undefined ? {} : { col }),
    })
  }
  return refs
}

/**
 * The checkout-relative path `ref.path` names, or null. Absolute paths must sit under
 * `checkoutPath`; relative paths try the session `cwd` first, then the checkout root, and a
 * result that climbs out of the checkout is dropped.
 */
export function resolveFileRef(
  ref: { path: string },
  { checkoutPath, cwd, files }: { checkoutPath: string; cwd: string | undefined; files: ReadonlySet<string> },
): string | null {
  const root = trimTrailingSlashes(checkoutPath)
  const prefix = root === '/' ? '/' : `${root}/`
  const hit = (rel: string | null) => (rel !== null && files.has(rel) ? rel : null)

  if (ref.path.startsWith('/')) {
    return ref.path.startsWith(prefix) ? hit(joinRelative('', ref.path.slice(prefix.length))) : null
  }
  if (cwd) {
    const base =
      cwd === root ? '' : cwd.startsWith(prefix) ? cwd.slice(prefix.length) : cwd.startsWith('/') ? null : trimTrailingSlashes(cwd)
    const normalized = base === null ? null : joinRelative('', base)
    if (normalized !== null) {
      const found = hit(joinRelative(normalized, ref.path))
      if (found !== null) return found
    }
  }
  return hit(joinRelative('', ref.path))
}
