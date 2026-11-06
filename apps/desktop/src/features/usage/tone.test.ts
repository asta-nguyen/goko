import { usageAge, usageTone, usageValue, windowLabel } from './tone'

const limit = (used_pct: number, resets_at = 200) => ({ used_pct, resets_at, window_minutes: 300 })

describe('usage indicators', () => {
  it.each([
    [79, 'muted'],
    [80, 'warning'],
    [89, 'warning'],
    [90, 'danger'],
  ] as const)('uses %s%% as %s', (percentage, tone) => {
    expect(usageTone(percentage)).toBe(tone)
  })

  it('shows reset instead of an expired percentage', () => {
    expect(usageValue(limit(42, 99), 100_000)).toEqual({ text: 'reset', tone: 'muted' })
  })

  it('labels windows from their duration', () => {
    expect(windowLabel(300, 'claude')).toBe('5h')
    expect(windowLabel(10_080, 'claude')).toBe('7d')
    expect(windowLabel(10_080, 'codex')).toBe('wk')
  })

  it('shows figure age and handles an unknown timestamp', () => {
    expect(usageAge(0, 90_000)).toBe('1m ago')
    expect(usageAge(null, 90_000)).toBe('unknown')
  })
})
