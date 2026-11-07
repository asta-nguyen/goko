import { fireEvent, render, screen } from '@testing-library/react'
import { useState } from 'react'
import { ResizableHandle } from './resizable-handle'

const separator = () => screen.getByRole('separator', { name: 'Resize test column' })

const Harness = ({
  orientation = 'vertical',
  initial = 100,
  invert = false,
  min = 56,
  max = 360,
  onReset = () => {},
}: {
  orientation?: 'vertical' | 'horizontal'
  initial?: number
  invert?: boolean
  min?: number
  max?: number
  onReset?: () => void
}) => {
  const [value, setValue] = useState(initial)
  return (
    <ResizableHandle
      orientation={orientation}
      label="Resize test column"
      value={value}
      min={min}
      max={max}
      invert={invert}
      onChange={setValue}
      onReset={onReset}
    />
  )
}

describe('ResizableHandle', () => {
  it('renders a focusable separator with the current value', () => {
    render(<Harness />)
    expect(separator().getAttribute('aria-orientation')).toBe('vertical')
    expect(separator().getAttribute('aria-valuemin')).toBe('56')
    expect(separator().getAttribute('aria-valuemax')).toBe('360')
    expect(separator().getAttribute('aria-valuenow')).toBe('100')
    expect(separator().tabIndex).toBe(0)
  })

  it('ArrowRight adds 16 and Shift adds 64', () => {
    render(<Harness />)
    fireEvent.keyDown(separator(), { key: 'ArrowRight' })
    expect(separator().getAttribute('aria-valuenow')).toBe('116')
    fireEvent.keyDown(separator(), { key: 'ArrowLeft' })
    expect(separator().getAttribute('aria-valuenow')).toBe('100')
    fireEvent.keyDown(separator(), { key: 'ArrowRight', shiftKey: true })
    expect(separator().getAttribute('aria-valuenow')).toBe('164')
  })

  it('Home/End hit min/max', () => {
    render(<Harness />)
    fireEvent.keyDown(separator(), { key: 'End' })
    expect(separator().getAttribute('aria-valuenow')).toBe('360')
    fireEvent.keyDown(separator(), { key: 'ArrowRight' })
    expect(separator().getAttribute('aria-valuenow')).toBe('360')
    fireEvent.keyDown(separator(), { key: 'Home' })
    expect(separator().getAttribute('aria-valuenow')).toBe('56')
    fireEvent.keyDown(separator(), { key: 'ArrowLeft' })
    expect(separator().getAttribute('aria-valuenow')).toBe('56')
  })

  it('drag clamps to the limits', () => {
    render(<Harness />)
    fireEvent.pointerDown(separator(), { pointerId: 1, clientX: 200, buttons: 1 })
    fireEvent.pointerMove(separator(), { pointerId: 1, clientX: 900, buttons: 1 })
    expect(separator().getAttribute('aria-valuenow')).toBe('360')
    fireEvent.pointerMove(separator(), { pointerId: 1, clientX: 0, buttons: 1 })
    expect(separator().getAttribute('aria-valuenow')).toBe('56')
    fireEvent.pointerMove(separator(), { pointerId: 1, clientX: 316, buttons: 1 })
    expect(separator().getAttribute('aria-valuenow')).toBe('216')
    fireEvent.pointerMove(separator(), { pointerId: 1, clientX: 400, buttons: 0 })
    expect(separator().getAttribute('aria-valuenow')).toBe('216')
    fireEvent.pointerUp(separator(), { pointerId: 1 })
    fireEvent.pointerMove(separator(), { pointerId: 1, clientX: 500, buttons: 1 })
    expect(separator().getAttribute('aria-valuenow')).toBe('216')
  })

  it('uses ArrowUp/ArrowDown for a horizontal handle', () => {
    render(<Harness orientation="horizontal" initial={320} min={160} max={600} />)
    expect(separator().getAttribute('aria-orientation')).toBe('horizontal')
    fireEvent.keyDown(separator(), { key: 'ArrowDown' })
    expect(separator().getAttribute('aria-valuenow')).toBe('336')
    fireEvent.keyDown(separator(), { key: 'ArrowUp', shiftKey: true })
    expect(separator().getAttribute('aria-valuenow')).toBe('272')
    fireEvent.keyDown(separator(), { key: 'ArrowRight' })
    expect(separator().getAttribute('aria-valuenow')).toBe('272')
    fireEvent.pointerDown(separator(), { pointerId: 1, clientY: 100, buttons: 1 })
    fireEvent.pointerMove(separator(), { pointerId: 1, clientY: 200, buttons: 1 })
    expect(separator().getAttribute('aria-valuenow')).toBe('372')
  })

  it('double-click calls onReset', () => {
    const onReset = vi.fn()
    render(<Harness onReset={onReset} />)
    fireEvent.doubleClick(separator())
    expect(onReset).toHaveBeenCalledTimes(1)
  })
  it('invert grows the value when the edge moves toward the origin', () => {
    render(<Harness invert />)
    fireEvent.keyDown(separator(), { key: 'ArrowLeft' })
    expect(separator().getAttribute('aria-valuenow')).toBe('116')
    fireEvent.keyDown(separator(), { key: 'ArrowRight', shiftKey: true })
    // 116 - 64 = 52, clamped to the minimum of 56.
    expect(separator().getAttribute('aria-valuenow')).toBe('56')
  })

  it('invert reverses the drag direction', () => {
    render(<Harness invert />)
    fireEvent.pointerDown(separator(), { clientX: 200, pointerId: 1 })
    fireEvent.pointerMove(separator(), { clientX: 170, buttons: 1, pointerId: 1 })
    expect(separator().getAttribute('aria-valuenow')).toBe('130')
  })
})
