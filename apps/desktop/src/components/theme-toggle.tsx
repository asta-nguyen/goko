import { MoonIcon, SunIcon } from 'lucide-react'
import { useEffect, useState } from 'react'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'

type Theme = 'dark' | 'light'

const STORAGE_KEY = 'goko.theme'

function storedTheme(): Theme {
  if (typeof window === 'undefined') return 'dark'
  return window.localStorage.getItem(STORAGE_KEY) === 'light' ? 'light' : 'dark'
}

function applyTheme(theme: Theme) {
  const root = document.documentElement
  if (theme === 'light') root.dataset.theme = 'light'
  else delete root.dataset.theme
}

export function ThemeToggle() {
  const [theme, setTheme] = useState<Theme>(storedTheme)

  useEffect(() => {
    applyTheme(theme)
    window.localStorage.setItem(STORAGE_KEY, theme)
  }, [theme])

  return (
    <ToggleGroup
      type="single"
      value={theme}
      onValueChange={(value) => {
        if (value === 'dark' || value === 'light') setTheme(value)
      }}
      variant="outline"
      size="sm"
      aria-label="Theme"
      className="gap-0"
    >
      <ToggleGroupItem value="dark" aria-label="Dark theme" title="Dark theme">
        <MoonIcon />
      </ToggleGroupItem>
      <ToggleGroupItem value="light" aria-label="Light theme" title="Light theme">
        <SunIcon />
      </ToggleGroupItem>
    </ToggleGroup>
  )
}
