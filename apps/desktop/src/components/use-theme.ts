import { useEffect, useSyncExternalStore } from 'react'

export type Theme = 'system' | 'dark' | 'light' | 'one-dark' | 'catppuccin-mocha' | 'dracula'
export type ResolvedTheme = Exclude<Theme, 'system'>

export const THEME_OPTIONS: { id: Theme; label: string }[] = [
  { id: 'system', label: 'System' },
  { id: 'dark', label: 'Dark+' },
  { id: 'light', label: 'Light' },
  { id: 'one-dark', label: 'One Dark' },
  { id: 'catppuccin-mocha', label: 'Catppuccin Mocha' },
  { id: 'dracula', label: 'Dracula' },
]

/** bg / accent / text preview colors shown next to each theme in Settings. */
export const THEME_SWATCH: Record<ResolvedTheme, [string, string, string]> = {
  dark: ['#1e1e1e', '#007acc', '#cccccc'],
  light: ['#f5f7fa', '#116c6b', '#1a2430'],
  'one-dark': ['#282c34', '#61afef', '#abb2bf'],
  'catppuccin-mocha': ['#1e1e2e', '#cba6f7', '#cdd6f4'],
  dracula: ['#282a36', '#bd93f9', '#f8f8f2'],
}

const STORAGE_KEY = 'goko.theme'
const THEME_IDS = new Set<string>(THEME_OPTIONS.map((option) => option.id))

function storedTheme(): Theme {
  if (typeof window === 'undefined') return 'dark'
  const value = window.localStorage.getItem(STORAGE_KEY)
  return THEME_IDS.has(value ?? '') ? (value as Theme) : 'dark'
}

export function resolveTheme(theme: Theme): ResolvedTheme {
  if (theme !== 'system') return theme
  return window.matchMedia?.('(prefers-color-scheme: light)').matches ? 'light' : 'dark'
}

function applyTheme(theme: Theme) {
  const resolved = resolveTheme(theme)
  const root = document.documentElement
  if (resolved === 'dark') delete root.dataset.theme
  else root.dataset.theme = resolved
}

// `goko.theme` in local storage is the source of truth; the version counter
// notifies every mounted control when one of them writes a new value.
let version = 0
const listeners = new Set<() => void>()

export function setTheme(theme: Theme) {
  window.localStorage.setItem(STORAGE_KEY, theme)
  version += 1
  listeners.forEach((listener) => listener())
}

// Apply the stored palette before React's first paint, and keep following the
// OS appearance while `system` is selected.
if (typeof window !== 'undefined') {
  applyTheme(storedTheme())
  window.matchMedia?.('(prefers-color-scheme: light)').addEventListener?.('change', () => {
    if (storedTheme() === 'system') applyTheme('system')
  })
}

export function useTheme(): readonly [Theme, typeof setTheme] {
  useSyncExternalStore(
    (listener) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    () => version,
  )
  const theme = storedTheme()
  useEffect(() => applyTheme(theme), [theme])
  return [theme, setTheme]
}
