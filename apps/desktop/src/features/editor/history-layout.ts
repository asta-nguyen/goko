import { useEffect, useState } from 'react'

const STORAGE_KEY = 'goko.history.layout'

/** Persisted History pane sizes in px; `graph` unset means the natural lane width. */
export interface HistoryLayout {
  graph?: number
  date: number
  author: number
  commit: number
  detailHeight: number
}

export type HistoryColumn = 'graph' | 'date' | 'author' | 'commit'

export const DEFAULT_LAYOUT: HistoryLayout = {
  date: 120,
  author: 128,
  commit: 80,
  detailHeight: 320,
}

export const LAYOUT_LIMITS = {
  graph: { min: 48, max: 320 },
  date: { min: 56, max: 360 },
  author: { min: 56, max: 360 },
  commit: { min: 56, max: 360 },
  detailHeight: { min: 160 },
} as const

const bounded = (value: unknown, limits: { min: number; max?: number }): number | null => {
  if (typeof value !== 'number' || !Number.isFinite(value)) return null
  const rounded = Math.round(value)
  return Math.min(limits.max ?? Number.POSITIVE_INFINITY, Math.max(limits.min, rounded))
}

const readLayout = (): HistoryLayout => {
  let raw: string | null
  try {
    raw = localStorage.getItem(STORAGE_KEY)
  } catch {
    return DEFAULT_LAYOUT
  }
  if (raw === null) return DEFAULT_LAYOUT
  try {
    const parsed: unknown = JSON.parse(raw)
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return DEFAULT_LAYOUT
    const record = parsed as Record<string, unknown>
    return {
      graph: bounded(record.graph, LAYOUT_LIMITS.graph) ?? undefined,
      date: bounded(record.date, LAYOUT_LIMITS.date) ?? DEFAULT_LAYOUT.date,
      author: bounded(record.author, LAYOUT_LIMITS.author) ?? DEFAULT_LAYOUT.author,
      commit: bounded(record.commit, LAYOUT_LIMITS.commit) ?? DEFAULT_LAYOUT.commit,
      detailHeight: bounded(record.detailHeight, LAYOUT_LIMITS.detailHeight) ?? DEFAULT_LAYOUT.detailHeight,
    }
  } catch {
    return DEFAULT_LAYOUT
  }
}

/** History column widths and detail height, persisted to localStorage. */
export function useHistoryLayout() {
  const [layout, setLayout] = useState<HistoryLayout>(readLayout)

  useEffect(() => {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(layout))
    } catch {
      // Storage is unavailable; the layout stays in memory for this run.
    }
  }, [layout])

  const setColumn = (name: HistoryColumn, px: number) =>
    setLayout((current) => {
      const value = bounded(px, LAYOUT_LIMITS[name])
      if (value === null) return current
      const next = { ...current }
      next[name] = value
      return next
    })

  const setDetailHeight = (px: number) =>
    setLayout((current) => {
      const value = bounded(px, LAYOUT_LIMITS.detailHeight)
      if (value === null) return current
      return { ...current, detailHeight: value }
    })

  const reset = (name: keyof HistoryLayout) =>
    setLayout((current) => {
      const next = { ...current }
      if (name === 'graph') delete next.graph
      else next[name] = DEFAULT_LAYOUT[name]
      return next
    })

  return { layout, setColumn, setDetailHeight, reset }
}
