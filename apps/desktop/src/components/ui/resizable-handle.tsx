import { useRef } from 'react'
import { cn } from '@/lib/utils'
import { Separator } from '@/components/ui/separator'

const DEFAULT_STEP = 16
const SHIFT_STEP_FACTOR = 4

const clamp = (value: number, min: number, max: number) => Math.max(min, Math.min(max, value))

/**
 * A draggable separator between two panes. `orientation="vertical"` is a
 * column edge adjusted with Left/Right; `"horizontal"` is a row divider
 * adjusted with Up/Down. Arrows move by `step` (×4 with Shift), Home/End hit
 * min/max, and double-click restores the default through `onReset`.
 */
export function ResizableHandle({
  orientation,
  label,
  value,
  min,
  max,
  step = DEFAULT_STEP,
  invert = false,
  onChange,
  onReset,
}: {
  /** Separator orientation: vertical for column edges, horizontal for row dividers. */
  orientation: 'vertical' | 'horizontal'
  /** The separator's accessible name, e.g. "Resize date column". */
  label: string
  value: number
  min: number
  max: number
  /** Arrow-key step in px; Shift multiplies it by four. */
  step?: number
  /** For an edge on the near side of the pane it sizes: moving the edge toward the origin grows the value. */
  invert?: boolean
  onChange: (value: number) => void
  onReset: () => void
}) {
  const drag = useRef<{ start: number; value: number } | null>(null)
  const current = clamp(value, min, max)
  const commit = (next: number) => onChange(clamp(Math.round(next), min, max))
  const sign = invert ? -1 : 1
  const position = (event: React.PointerEvent<HTMLDivElement>) =>
    orientation === 'vertical' ? event.clientX : event.clientY

  const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const delta = event.shiftKey ? step * SHIFT_STEP_FACTOR : step
    const vertical = orientation === 'vertical'
    let next: number | null = null
    if (vertical && event.key === 'ArrowLeft') next = current - sign * delta
    if (vertical && event.key === 'ArrowRight') next = current + sign * delta
    if (!vertical && event.key === 'ArrowUp') next = current - sign * delta
    if (!vertical && event.key === 'ArrowDown') next = current + sign * delta
    if (event.key === 'Home') next = min
    if (event.key === 'End') next = max
    if (next === null) return
    event.preventDefault()
    commit(next)
  }

  return (
    <div
      role="separator"
      aria-label={label}
      aria-orientation={orientation}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuenow={current}
      tabIndex={0}
      onKeyDown={onKeyDown}
      onDoubleClick={onReset}
      onPointerDown={(event) => {
        event.preventDefault()
        event.currentTarget.setPointerCapture?.(event.pointerId)
        drag.current = { start: position(event), value: current }
      }}
      onPointerMove={(event) => {
        if (!drag.current || event.buttons !== 1) return
        commit(drag.current.value + sign * (position(event) - drag.current.start))
      }}
      onPointerUp={() => {
        drag.current = null
      }}
      onPointerCancel={() => {
        drag.current = null
      }}
      className={cn(
        'group relative z-10 shrink-0 touch-none bg-transparent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/50',
        orientation === 'vertical' ? 'w-2 cursor-col-resize' : 'h-2 cursor-row-resize',
      )}
    >
      <Separator
        orientation={orientation}
        className={cn(
          'absolute transition-colors group-hover:bg-accent group-focus-visible:bg-accent',
          orientation === 'vertical'
            ? 'inset-y-0 left-1/2 h-full -translate-x-1/2'
            : 'inset-x-0 top-1/2 -translate-y-1/2',
        )}
      />
    </div>
  )
}
