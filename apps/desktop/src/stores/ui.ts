import { create } from 'zustand'
import { useSessionList } from '../features/sessions/queries'

// UI-only state. Daemon data lives in the query cache, terminal bytes nowhere in React.
interface UiState {
  selectedId: string | null
  select: (id: string | null) => void
}

export const useUi = create<UiState>((set) => ({
  selectedId: null,
  select: (selectedId) => set({ selectedId }),
}))

/** The stored id only while that session is in the list, so nothing needs clearing when one vanishes. */
export function useSelectedId(): string | null {
  const id = useUi((s) => s.selectedId)
  const { data } = useSessionList()
  return id !== null && data?.some((s) => s.id === id) ? id : null
}
