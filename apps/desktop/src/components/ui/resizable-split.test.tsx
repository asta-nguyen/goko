import { act, fireEvent, render, screen } from '@testing-library/react'
import { useState } from 'react'
import { ResizableSplit } from './resizable-split'

let resizeCallbacks: (() => void)[]
const separator = () => screen.getByRole('separator', { name: 'Resize test panel' })
const aside = () => screen.getByRole('complementary')

const Harness = ({
  min = 180,
  max = 480,
  siblingMin = 320,
  initial = 288,
}: {
  min?: number
  max?: number
  siblingMin?: number
  initial?: number
}) => {
  const [width, setWidth] = useState(initial)
  return (
    <ResizableSplit
      label="Resize test panel"
      width={width}
      min={min}
      max={max}
      siblingMin={siblingMin}
      onResize={setWidth}
      aside={<p>list</p>}
    >
      <p>detail</p>
    </ResizableSplit>
  )
}

beforeEach(() => {
  resizeCallbacks = []
  vi.stubGlobal('ResizeObserver', class {
    constructor(callback: () => void) {
      resizeCallbacks.push(callback)
    }
    observe() {}
    disconnect() {}
  })
})
afterEach(() => vi.unstubAllGlobals())

const mockWidth = (px: number) => {
  const container = separator().parentElement!
  vi.spyOn(container, 'getBoundingClientRect').mockReturnValue({
    x: 0, y: 0, left: 0, top: 0, right: px, bottom: 600, width: px, height: 600, toJSON: () => ({}),
  })
  act(() => resizeCallbacks.forEach((callback) => callback()))
}

describe('ResizableSplit', () => {
  it('renders a focusable vertical separator with the bounded width', () => {
    render(<Harness />)
    expect(separator().getAttribute('aria-orientation')).toBe('vertical')
    expect(separator().getAttribute('aria-valuemin')).toBe('180')
    expect(separator().getAttribute('aria-valuemax')).toBe('480')
    expect(separator().getAttribute('aria-valuenow')).toBe('288')
    expect(separator().tabIndex).toBe(0)
    expect(separator().className).toContain('focus-visible:ring-2')
    expect(aside().style.width).toBe('288px')
  })

  it('resizes with arrow keys, Home, and End', () => {
    render(<Harness />)
    fireEvent.keyDown(separator(), { key: 'ArrowRight' })
    expect(separator().getAttribute('aria-valuenow')).toBe('304')
    fireEvent.keyDown(separator(), { key: 'ArrowLeft' })
    expect(separator().getAttribute('aria-valuenow')).toBe('288')
    fireEvent.keyDown(separator(), { key: 'ArrowRight', shiftKey: true })
    expect(separator().getAttribute('aria-valuenow')).toBe('352')
    fireEvent.keyDown(separator(), { key: 'Home' })
    expect(separator().getAttribute('aria-valuenow')).toBe('180')
    fireEvent.keyDown(separator(), { key: 'ArrowLeft' })
    expect(separator().getAttribute('aria-valuenow')).toBe('180')
    fireEvent.keyDown(separator(), { key: 'End' })
    expect(separator().getAttribute('aria-valuenow')).toBe('480')
    fireEvent.keyDown(separator(), { key: 'ArrowRight' })
    expect(separator().getAttribute('aria-valuenow')).toBe('480')
  })

  it('clamps pointer drags inside the same bounds', () => {
    render(<Harness />)
    mockWidth(900)
    fireEvent.pointerDown(separator(), { pointerId: 1, clientX: 700, buttons: 1 })
    expect(separator().getAttribute('aria-valuenow')).toBe('480')
    fireEvent.pointerMove(separator(), { pointerId: 1, clientX: 100, buttons: 1 })
    expect(separator().getAttribute('aria-valuenow')).toBe('180')
    fireEvent.pointerMove(separator(), { pointerId: 1, clientX: 300, buttons: 1 })
    expect(separator().getAttribute('aria-valuenow')).toBe('296')
    fireEvent.pointerMove(separator(), { pointerId: 1, clientX: 50, buttons: 0 })
    expect(separator().getAttribute('aria-valuenow')).toBe('296')
  })

  it('shrinks the maximum so the sibling keeps its minimum', () => {
    render(<Harness />)
    mockWidth(700)
    expect(separator().getAttribute('aria-valuemax')).toBe('372')
    fireEvent.keyDown(separator(), { key: 'End' })
    expect(separator().getAttribute('aria-valuenow')).toBe('372')
  })

  it('keeps both minimums and scrolls when the row cannot fit them', () => {
    render(<Harness />)
    mockWidth(400)
    expect(separator().getAttribute('aria-valuemax')).toBe('180')
    expect(separator().getAttribute('aria-valuenow')).toBe('180')
    expect(aside().style.width).toBe('180px')
    expect(screen.getByText('detail').parentElement!.style.minWidth).toBe('320px')
    expect(separator().parentElement!.className).toContain('overflow-x-auto')
  })
})
