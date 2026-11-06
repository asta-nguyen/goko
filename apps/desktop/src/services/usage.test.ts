import { claudeSessionId } from './usage'

describe('claudeSessionId', () => {
  it('returns the id after --session-id', () => {
    expect(claudeSessionId(['claude', '--session-id', 'u1', '--settings', '{}'])).toBe('u1')
  })

  it('returns null without the flag or its value', () => {
    expect(claudeSessionId(['claude', '--settings', '{}'])).toBeNull()
    expect(claudeSessionId(['claude', '--session-id'])).toBeNull()
  })
})
