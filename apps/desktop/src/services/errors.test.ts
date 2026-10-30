import { isBridgeError, toBridgeError } from './errors'

describe('toBridgeError', () => {
  it('keeps a bridge error as that union member', () => {
    const e = toBridgeError({ code: 'auth_failed', message: 'bad token', hint: 'restart goko' })
    expect(e).toEqual({ code: 'auth_failed', message: 'bad token', hint: 'restart goko' })
    expect(isBridgeError(e)).toBe(true)
  })

  it('fills a missing hint with null', () => {
    expect(toBridgeError({ code: 'timeout', message: 'slow' })).toEqual({ code: 'timeout', message: 'slow', hint: null })
  })

  it.each([['boom', 'boom'], [undefined, 'Unknown error'], [null, 'Unknown error'], [42, 'Unknown error']])(
    'maps %j to unknown',
    (input, message) => {
      expect(toBridgeError(input)).toEqual({ code: 'unknown', message, hint: null })
    },
  )

  it('maps an object with an unrecognised code to unknown but keeps its message', () => {
    expect(toBridgeError({ code: 'nope', message: 'hm' })).toEqual({ code: 'unknown', message: 'hm', hint: null })
    expect(toBridgeError(new Error('js error'))).toEqual({ code: 'unknown', message: 'js error', hint: null })
  })
})

describe('isBridgeError', () => {
  it('rejects non-bridge values', () => {
    expect(isBridgeError('x')).toBe(false)
    expect(isBridgeError(null)).toBe(false)
    expect(isBridgeError({ code: 'nope', message: 'm', hint: null })).toBe(false)
  })
})
