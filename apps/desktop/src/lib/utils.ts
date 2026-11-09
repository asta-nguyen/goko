import { clsx, type ClassValue } from 'clsx'
import { twMerge } from 'tailwind-merge'

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

// Ctrl+click is a right click on macOS, so go-to there is Cmd+click.
export const goToModifier = (event: MouseEvent) => (/Macintosh/.test(navigator.userAgent) ? event.metaKey : event.ctrlKey)
