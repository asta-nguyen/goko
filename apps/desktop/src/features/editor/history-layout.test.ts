import { act, renderHook } from '@testing-library/react'
import { DEFAULT_LAYOUT, LAYOUT_LIMITS, useHistoryLayout } from './history-layout'

const KEY = 'goko.history.layout'

beforeEach(() => localStorage.clear())
afterEach(() => vi.unstubAllGlobals())

describe('useHistoryLayout', () => {
  it('defaults without storage', () => {
    const { result } = renderHook(() => useHistoryLayout())
    expect(result.current.layout).toEqual(DEFAULT_LAYOUT)
    expect(result.current.layout.graph).toBeUndefined()
    expect(result.current.layout.date).toBe(120)
  })

  it('invalid JSON falls back to defaults', () => {
    localStorage.setItem(KEY, 'not json {')
    const { result } = renderHook(() => useHistoryLayout())
    expect(result.current.layout).toEqual(DEFAULT_LAYOUT)

    localStorage.setItem(KEY, '42')
    const second = renderHook(() => useHistoryLayout())
    expect(second.result.current.layout).toEqual(DEFAULT_LAYOUT)
  })

  it('out-of-range values are clamped', () => {
    localStorage.setItem(
      KEY,
      JSON.stringify({
        graph: 500,
        date: 9999,
        author: 10,
        commit: 200,
        detailHeight: 5,
      }),
    )
    const { result } = renderHook(() => useHistoryLayout())
    expect(result.current.layout.graph).toBe(LAYOUT_LIMITS.graph.max)
    expect(result.current.layout.date).toBe(LAYOUT_LIMITS.date.max)
    expect(result.current.layout.author).toBe(LAYOUT_LIMITS.author.min)
    expect(result.current.layout.commit).toBe(200)
    expect(result.current.layout.detailHeight).toBe(LAYOUT_LIMITS.detailHeight.min)

    localStorage.setItem(KEY, JSON.stringify({ date: 'wide', detailHeight: 'tall' }))
    const bad = renderHook(() => useHistoryLayout())
    expect(bad.result.current.layout.date).toBe(120)
    expect(bad.result.current.layout.detailHeight).toBe(320)
  })

  it('getItem/setItem that throw do not break the hook', () => {
    vi.stubGlobal('localStorage', {
      getItem: () => {
        throw new Error('denied')
      },
      setItem: () => {
        throw new Error('denied')
      },
      removeItem: () => {},
      clear: () => {},
      key: () => null,
      length: 0,
    })
    const { result } = renderHook(() => useHistoryLayout())
    expect(result.current.layout).toEqual(DEFAULT_LAYOUT)
    act(() => result.current.setColumn('date', 200))
    expect(result.current.layout.date).toBe(200)
    act(() => result.current.setDetailHeight(400))
    expect(result.current.layout.detailHeight).toBe(400)
    act(() => result.current.reset('date'))
    expect(result.current.layout.date).toBe(120)
  })

  it('a change is written to storage', () => {
    const { result } = renderHook(() => useHistoryLayout())
    act(() => result.current.setColumn('date', 200))
    expect(result.current.layout.date).toBe(200)
    let stored = JSON.parse(localStorage.getItem(KEY)!)
    expect(stored.date).toBe(200)
    expect(stored.graph).toBeUndefined()

    act(() => result.current.setColumn('author', 9999))
    stored = JSON.parse(localStorage.getItem(KEY)!)
    expect(stored.author).toBe(360)

    act(() => result.current.setColumn('graph', 140))
    stored = JSON.parse(localStorage.getItem(KEY)!)
    expect(stored.graph).toBe(140)

    act(() => result.current.setDetailHeight(500))
    stored = JSON.parse(localStorage.getItem(KEY)!)
    expect(stored.detailHeight).toBe(500)
  })

  it('reset restores one column', () => {
    const { result } = renderHook(() => useHistoryLayout())
    act(() => result.current.setColumn('date', 200))
    act(() => result.current.setColumn('author', 300))
    act(() => result.current.setColumn('graph', 140))
    act(() => result.current.reset('date'))
    expect(result.current.layout.date).toBe(120)
    expect(result.current.layout.author).toBe(300)
    expect(result.current.layout.graph).toBe(140)

    act(() => result.current.reset('graph'))
    expect(result.current.layout.graph).toBeUndefined()
    const stored = JSON.parse(localStorage.getItem(KEY)!)
    expect(stored.date).toBe(120)
    expect(stored.graph).toBeUndefined()

    act(() => result.current.setDetailHeight(500))
    act(() => result.current.reset('detailHeight'))
    expect(result.current.layout.detailHeight).toBe(320)
  })
})
