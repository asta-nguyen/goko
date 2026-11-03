import { useEffect, useRef, useState } from 'react'
import { cn } from '@/lib/utils'
import { Separator } from '@/components/ui/separator'

const SEPARATOR_SIZE = 8
const ARROW_STEP = 16
const ARROW_STEP_SHIFT = 64

/**
 * A left/right split whose first pane has a bounded pixel width. The maximum
 * shrinks so the second pane keeps `siblingMin`; when the row cannot fit both
 * minimums it keeps them and scrolls horizontally. Pointer drag and
 * Left/Right, Home, and End adjust the width.
 */
export function ResizableSplit({
  label,
  width,
  min,
  max,
  siblingMin,
  onResize,
  aside,
  asideClassName,
  className,
  children,
}: {
  /** The separator's accessible name, e.g. "Resize project sidebar". */
  label: string
  width: number
  min: number
  max: number
  /** The second pane's minimum width; caps `max` as the row narrows. */
  siblingMin: number
  onResize: (width: number) => void
  aside: React.ReactNode
  asideClassName?: string
  className?: string
  children: React.ReactNode
}) {
  const container = useRef<HTMLDivElement>(null)
  const [available, setAvailable] = useState<number | null>(null)

  useEffect(() => {
    const update = () => {
      const dimension = container.current?.getBoundingClientRect().width
      if (dimension) setAvailable(Math.round(dimension))
    }
    update()
    if (!container.current) return
    const observer = new ResizeObserver(update)
    observer.observe(container.current)
    return () => observer.disconnect()
  }, [])

  const maxBound =
    available === null ? max : Math.max(min, Math.min(max, available - siblingMin - SEPARATOR_SIZE))
  const effective = Math.max(min, Math.min(maxBound, width))
  const resize = (next: number) => onResize(Math.max(min, Math.min(maxBound, Math.round(next))))

  const widthAt = (clientX: number) => {
    const element = container.current
    const rect = element?.getBoundingClientRect()
    if (!element || !rect) return
    resize(clientX - rect.left + element.scrollLeft - SEPARATOR_SIZE / 2)
  }

  const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const step = event.shiftKey ? ARROW_STEP_SHIFT : ARROW_STEP
    let next: number | null = null
    if (event.key === 'ArrowLeft') next = effective - step
    if (event.key === 'ArrowRight') next = effective + step
    if (event.key === 'Home') next = min
    if (event.key === 'End') next = maxBound
    if (next === null) return
    event.preventDefault()
    resize(next)
  }

  return (
    <div ref={container} className={cn('flex min-h-0 min-w-0 flex-1 overflow-x-auto', className)}>
      <aside
        className={cn('flex min-h-0 shrink-0 flex-col overflow-hidden', asideClassName)}
        style={{ width: effective }}
      >
        {aside}
      </aside>
      <div
        role="separator"
        aria-label={label}
        aria-orientation="vertical"
        aria-valuemin={min}
        aria-valuemax={maxBound}
        aria-valuenow={effective}
        tabIndex={0}
        onKeyDown={onKeyDown}
        onPointerDown={(event) => {
          event.preventDefault()
          event.currentTarget.setPointerCapture?.(event.pointerId)
          widthAt(event.clientX)
        }}
        onPointerMove={(event) => {
          if (event.buttons !== 1) return
          widthAt(event.clientX)
        }}
        className="group relative z-10 w-2 shrink-0 cursor-col-resize touch-none bg-transparent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/50"
      >
        <Separator
          orientation="vertical"
          className="absolute inset-y-0 left-1/2 h-full -translate-x-1/2 transition-colors group-hover:bg-accent group-focus-visible:bg-accent"
        />
      </div>
      <div className="flex flex-1 flex-col overflow-hidden" style={{ minWidth: siblingMin }}>
        {children}
      </div>
    </div>
  )
}
