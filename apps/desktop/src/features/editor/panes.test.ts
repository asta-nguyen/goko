import { matchesQuery } from './panes'

describe('matchesQuery', () => {
  it('matches case-insensitive substrings', () => {
    expect(matchesQuery('src/features/editor/panes.tsx', 'editor')).toBe(true)
    expect(matchesQuery('src/features/editor/panes.tsx', 'EDITOR')).toBe(true)
    expect(matchesQuery('src/features/editor/panes.tsx', 'terminal')).toBe(false)
  })

  it('requires every whitespace-separated term', () => {
    expect(matchesQuery('src/features/editor/panes.tsx', 'src panes')).toBe(true)
    expect(matchesQuery('src/features/editor/panes.tsx', 'src terminal')).toBe(false)
  })

  it('matches everything for an empty or whitespace query', () => {
    expect(matchesQuery('anything', '')).toBe(true)
    expect(matchesQuery('anything', '   ')).toBe(true)
  })
})
