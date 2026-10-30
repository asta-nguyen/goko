const CODES = [
  'daemon_unavailable',
  'daemon_start_failed',
  'auth_failed',
  'version_unsupported',
  'timeout',
  'disconnected',
  // daemon `ErrorCode`s passed through by the bridge
  'frame_too_large',
  'malformed',
  'invalid_argument',
  'not_found',
  'spawn_failed',
  'internal',
  'unknown',
] as const

type Code = (typeof CODES)[number]

/** Discriminated union on `code`: one member per code. */
export type BridgeError = { [C in Code]: { code: C; message: string; hint: string | null } }[Code]

const isCode = (c: unknown): c is Code => CODES.includes(c as Code)

export function isBridgeError(e: unknown): e is BridgeError {
  if (typeof e !== 'object' || e === null) return false
  const o = e as Record<string, unknown>
  return isCode(o.code) && typeof o.message === 'string' && (o.hint === null || typeof o.hint === 'string')
}

/** Normalizes any rejected `invoke` value; never throws. */
export function toBridgeError(e: unknown): BridgeError {
  if (isBridgeError(e)) return e
  if (typeof e === 'object' && e !== null) {
    const o = e as Record<string, unknown>
    const code = isCode(o.code) ? o.code : 'unknown'
    const message = typeof o.message === 'string' ? o.message : 'Unknown error'
    return { code, message, hint: typeof o.hint === 'string' ? o.hint : null } as BridgeError
  }
  return { code: 'unknown', message: typeof e === 'string' ? e : 'Unknown error', hint: null }
}
