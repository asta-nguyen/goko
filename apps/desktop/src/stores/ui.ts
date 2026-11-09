import { create } from 'zustand'
import { useSessionList } from '../features/sessions/queries'

export const UNASSIGNED_WORKSPACE = '__unassigned__'
const CHECKOUT_KEY_SEPARATOR = '\u0000'

export function checkoutKey(projectId: string, checkoutPath: string): string {
  return `${projectId}${CHECKOUT_KEY_SEPARATOR}${checkoutPath}`
}

export function checkoutContext(workspaceId: string): { projectId: string; checkoutPath: string } | null {
  const separator = workspaceId.indexOf(CHECKOUT_KEY_SEPARATOR)
  if (separator < 0) return null
  return {
    projectId: workspaceId.slice(0, separator),
    checkoutPath: workspaceId.slice(separator + 1),
  }
}

function projectIdForWorkspace(workspaceId: string): string | null {
  const separator = workspaceId.indexOf(CHECKOUT_KEY_SEPARATOR)
  return separator < 0 ? (workspaceId === UNASSIGNED_WORKSPACE ? null : workspaceId) : workspaceId.slice(0, separator)
}

export function checkoutForCwd(
  checkouts: Array<{ path: string }>,
  cwd: string,
): string | null {
  const normalize = (path: string) => path.length > 1 ? path.replace(/\/+$/, '') : path
  const normalizedCwd = normalize(cwd)
  return checkouts
    .map(({ path }) => normalize(path))
    .filter((path) => normalizedCwd === path || normalizedCwd.startsWith(path.endsWith('/') ? path : `${path}/`))
    .sort((left, right) => right.length - left.length)[0] ?? null
}

export interface TerminalPane {
  kind: 'pane'
  id: string
  sessionId: string
  closingAt: number | null
  stopRequestedAt: number | null
  closeError: string | null
}

export interface TerminalSplit {
  kind: 'split'
  id: string
  direction: 'horizontal' | 'vertical'
  ratio: number
  first: PaneLayout
  second: PaneLayout
}

export type PaneLayout = TerminalPane | TerminalSplit

export interface TerminalTab {
  id: string
  title: string
  layout: PaneLayout
  activePaneId: string
  closing: boolean
}

export interface TerminalWorkspaceState {
  tabs: TerminalTab[]
  activeTabId: string | null
  defaultPending: boolean
  error: string | null
}

/** The main-area mode of the active workspace; never persisted. */
export type WorkspaceView = 'terminal' | 'editor' | 'settings'
export type DiffView = 'unified' | 'split'

export interface EditorDraft {
  projectId: string
  checkoutPath: string
  path: string
  originalText: string
  text: string
}

/** A one-shot request for the Editor to open a checkout file, e.g. from a terminal link. */
export interface EditorOpenRequest {
  workspaceId: string
  path: string
  line?: number
  nonce: number
}

type EditorNavigation = () => void | Promise<void>

/** Fixed app panel bounds in px; the widths live only in memory for the app run. */
export const PANEL_LIMITS = {
  projectTree: { defaultWidth: 320, minWidth: 220, maxWidth: 480, siblingMin: 320 },
  editorList: { defaultWidth: 288, minWidth: 180, maxWidth: 480, siblingMin: 320 },
} as const
export type PanelId = keyof typeof PANEL_LIMITS

interface UiState {
  selectedId: string | null
  activeWorkspaceId: string | null
  activeView: WorkspaceView
  /** One-shot Settings section request; SettingsPage selects it and clears it. */
  settingsSection: string | null
  editorDraft: EditorDraft | null
  editorGuardOpen: boolean
  editorSaveError: string | null
  pendingEditorNavigation: EditorNavigation | null
  editorSaveDraft: (() => Promise<boolean>) | null
  editorReloadDraft: (() => Promise<boolean>) | null
  editorOpenRequest: EditorOpenRequest | null
  workspaces: Record<string, TerminalWorkspaceState>
  removedProjectIds: string[]
  panelWidths: Record<PanelId, number>
  setPanelWidth: (panel: PanelId, width: number) => void
  diffView: DiffView
  setDiffView: (view: DiffView) => void
  select: (id: string | null) => void
  activateWorkspace: (id: string) => void
  setActiveView: (view: WorkspaceView) => void
  openSettings: (section?: string) => void
  clearSettingsSection: () => void
  setEditorDraft: (draft: EditorDraft | null) => void
  setEditorSaveError: (error: string | null) => void
  setEditorSaveDraft: (save: (() => Promise<boolean>) | null) => void
  setEditorReloadDraft: (reload: (() => Promise<boolean>) | null) => void
  requestEditorNavigation: (navigation: EditorNavigation) => void
  continueEditorNavigation: (discard: boolean) => void
  cancelEditorNavigation: () => void
  openEditorFile: (workspaceId: string, path: string, line?: number) => void
  clearEditorOpenRequest: () => void
  claimDefaultTab: (id: string) => boolean
  failDefaultTab: (id: string, error: string) => void
  openSession: (workspaceId: string, sessionId: string, title: string, activate?: boolean) => void
  setActiveTab: (workspaceId: string, tabId: string) => void
  focusPane: (workspaceId: string, tabId: string, paneId: string) => void
  splitPane: (workspaceId: string, tabId: string, paneId: string, direction: TerminalSplit['direction'], sessionId: string) => boolean
  setSplitRatio: (workspaceId: string, tabId: string, splitId: string, ratio: number) => void
  beginClosePane: (workspaceId: string, tabId: string, paneId: string, at: number) => void
  beginCloseTab: (workspaceId: string, tabId: string, at: number) => void
  markStopRequested: (sessionId: string, at: number) => void
  retryClose: (sessionId: string) => void
  setCloseError: (sessionId: string, error: string | null) => void
  removeSession: (sessionId: string) => void
  removeWorkspace: (workspaceId: string) => void
  removeProject: (projectId: string) => void
  reset: () => void
}

const emptyWorkspace = (): TerminalWorkspaceState => ({
  tabs: [],
  activeTabId: null,
  defaultPending: false,
  error: null,
})

const pane = (sessionId: string): TerminalPane => ({
  kind: 'pane',
  id: `pane-${sessionId}`,
  sessionId,
  closingAt: null,
  stopRequestedAt: null,
  closeError: null,
})

function collectPanes(layout: PaneLayout): TerminalPane[] {
  return layout.kind === 'pane' ? [layout] : [...collectPanes(layout.first), ...collectPanes(layout.second)]
}

export function focusedSession(workspace?: TerminalWorkspaceState): string | null {
  const tab = workspace?.tabs.find((item) => item.id === workspace.activeTabId)
  return tab ? collectPanes(tab.layout).find((item) => item.id === tab.activePaneId)?.sessionId ?? null : null
}

function updateTree(
  layout: PaneLayout,
  paneId: string,
  update: (pane: TerminalPane) => TerminalPane,
): [PaneLayout, boolean] {
  if (layout.kind === 'pane') return layout.id === paneId ? [update(layout), true] : [layout, false]
  const [first, foundFirst] = updateTree(layout.first, paneId, update)
  if (foundFirst) return [first === layout.first ? layout : { ...layout, first }, true]
  const [second, foundSecond] = updateTree(layout.second, paneId, update)
  return foundSecond
    ? [second === layout.second ? layout : { ...layout, second }, true]
    : [layout, false]
}

function splitTree(
  layout: PaneLayout,
  paneId: string,
  direction: TerminalSplit['direction'],
  sessionId: string,
): [PaneLayout, string | null] {
  if (layout.kind === 'pane') {
    if (layout.id !== paneId || layout.closingAt !== null) return [layout, null]
    const added = pane(sessionId)
    return [
      {
        kind: 'split',
        id: `split-${added.id}`,
        direction,
        ratio: 0.5,
        first: layout,
        second: added,
      },
      added.id,
    ]
  }
  const [first, firstId] = splitTree(layout.first, paneId, direction, sessionId)
  if (firstId) return [{ ...layout, first }, firstId]
  const [second, secondId] = splitTree(layout.second, paneId, direction, sessionId)
  return secondId ? [{ ...layout, second }, secondId] : [layout, null]
}

function removePane(layout: PaneLayout, sessionId: string): PaneLayout | null {
  if (layout.kind === 'pane') return layout.sessionId === sessionId ? null : layout
  const first = removePane(layout.first, sessionId)
  const second = removePane(layout.second, sessionId)
  if (!first) return second
  if (!second) return first
  return first === layout.first && second === layout.second ? layout : { ...layout, first, second }
}

function sessionLocation(workspaces: Record<string, TerminalWorkspaceState>, sessionId: string) {
  for (const [workspaceId, workspace] of Object.entries(workspaces)) {
    for (const tab of workspace.tabs) {
      const found = collectPanes(tab.layout).find((item) => item.sessionId === sessionId)
      if (found) return { workspaceId, tabId: tab.id, paneId: found.id }
    }
  }
  return null
}

function updateSession(
  workspaces: Record<string, TerminalWorkspaceState>,
  sessionId: string,
  update: (pane: TerminalPane) => TerminalPane,
) {
  const location = sessionLocation(workspaces, sessionId)
  const workspace = location && workspaces[location.workspaceId]
  if (!location || !workspace) return workspaces

  let changed = false
  const tabs = workspace.tabs.map((tab) => {
    if (tab.id !== location.tabId) return tab
    const [layout, found] = updateTree(tab.layout, location.paneId, update)
    if (!found || layout === tab.layout) return tab
    changed = true
    return { ...tab, layout }
  })
  return changed
    ? { ...workspaces, [location.workspaceId]: { ...workspace, tabs } }
    : workspaces
}

function setActive(workspaces: Record<string, TerminalWorkspaceState>, workspaceId: string, tabId: string, paneId?: string) {
  const workspace = workspaces[workspaceId]
  if (!workspace) return workspaces
  return {
    ...workspaces,
    [workspaceId]: {
      ...workspace,
      activeTabId: tabId,
      tabs: workspace.tabs.map((tab) =>
        tab.id === tabId && paneId ? { ...tab, activePaneId: paneId } : tab,
      ),
    },
  }
}

let editorOpenNonce = 0

const initialState = {
  selectedId: null,
  activeWorkspaceId: null,
  activeView: 'terminal' as WorkspaceView,
  settingsSection: null as string | null,
  editorDraft: null as EditorDraft | null,
  editorGuardOpen: false,
  editorSaveError: null as string | null,
  pendingEditorNavigation: null as EditorNavigation | null,
  editorSaveDraft: null as (() => Promise<boolean>) | null,
  editorReloadDraft: null as (() => Promise<boolean>) | null,
  editorOpenRequest: null as EditorOpenRequest | null,
  workspaces: {} as Record<string, TerminalWorkspaceState>,
  removedProjectIds: [] as string[],
  panelWidths: {
    projectTree: PANEL_LIMITS.projectTree.defaultWidth,
    editorList: PANEL_LIMITS.editorList.defaultWidth,
  },
  diffView: 'unified' as DiffView,
}

export const useUi = create<UiState>((set, get) => ({
  ...initialState,
  select: (selectedId) => set({ selectedId }),
  setPanelWidth: (panel, width) =>
    set((state) => ({ panelWidths: { ...state.panelWidths, [panel]: width } })),
  setDiffView: (diffView) => set({ diffView }),
  activateWorkspace: (id) => {
    // Picking a workspace is the way out of Settings, even the one already active.
    if (get().activeWorkspaceId === id && get().activeView !== 'settings') return
    get().requestEditorNavigation(() =>
      set((state) => {
        const workspaces = state.workspaces[id] ? state.workspaces : { ...state.workspaces, [id]: emptyWorkspace() }
        const activeView = state.activeView === 'settings' ? 'terminal' : state.activeView
        return { activeWorkspaceId: id, selectedId: focusedSession(workspaces[id]), workspaces, activeView }
      }),
    )
  },
  setActiveView: (activeView) => {
    if (get().activeView === activeView) return
    get().requestEditorNavigation(() => set({ activeView }))
  },
  // Unlike setActiveView this never early-returns: called while Settings is
  // already open it still switches the requested section.
  openSettings: (section) => {
    get().requestEditorNavigation(() =>
      set({ activeView: 'settings', settingsSection: section ?? null }),
    )
  },
  clearSettingsSection: () => set({ settingsSection: null }),
  setEditorDraft: (draft) => set({
    editorDraft: draft && draft.text !== draft.originalText.replace(/\r\n|\r/g, '\n') ? draft : null,
    editorSaveError: null,
  }),
  setEditorSaveError: (editorSaveError) => set({ editorSaveError }),
  setEditorSaveDraft: (editorSaveDraft) => set({ editorSaveDraft }),
  setEditorReloadDraft: (editorReloadDraft) => set({ editorReloadDraft }),
  requestEditorNavigation: (navigation) => {
    const state = get()
    if (!state.editorDraft) {
      void navigation()
      return
    }
    const previous = state.pendingEditorNavigation
    set({
      editorGuardOpen: true,
      editorSaveError: null,
      pendingEditorNavigation: previous
        ? () => Promise.resolve(previous()).then(navigation)
        : navigation,
    })
  },
  continueEditorNavigation: (discard) => {
    const navigation = get().pendingEditorNavigation
    set((state) => ({
      editorDraft: discard ? null : state.editorDraft,
      editorGuardOpen: false,
      editorSaveError: null,
      pendingEditorNavigation: null,
    }))
    if (navigation) void navigation()
  },
  cancelEditorNavigation: () =>
    set({ editorGuardOpen: false, pendingEditorNavigation: null, editorOpenRequest: null }),
  openEditorFile: (workspaceId, path, line) => {
    set({ editorOpenRequest: { workspaceId, path, line, nonce: ++editorOpenNonce } })
    get().setActiveView('editor')
  },
  clearEditorOpenRequest: () => set({ editorOpenRequest: null }),
  claimDefaultTab: (id) => {
    const current = get().workspaces[id] ?? emptyWorkspace()
    if (current.tabs.length > 0 || current.defaultPending) return false
    set((state) => ({
      workspaces: {
        ...state.workspaces,
        [id]: { ...(state.workspaces[id] ?? emptyWorkspace()), defaultPending: true, error: null },
      },
    }))
    return true
  },
  failDefaultTab: (id, error) =>
    set((state) => ({
      workspaces: {
        ...state.workspaces,
        [id]: { ...(state.workspaces[id] ?? emptyWorkspace()), defaultPending: false, error },
      },
    })),
  openSession: (requestedWorkspaceId, sessionId, title, activate = true) => {
    const state = get()
    const location = sessionLocation(state.workspaces, sessionId)
    const projectId = projectIdForWorkspace(requestedWorkspaceId)
    const projectWasRemoved = projectId !== null && state.removedProjectIds.includes(projectId)
    const willActivate = location ? activate : activate || (projectWasRemoved && state.activeWorkspaceId === null)
    const open = () => set((state) => {
      const location = sessionLocation(state.workspaces, sessionId)
      if (location) {
        if (!activate) return state
        const workspaces = setActive(state.workspaces, location.workspaceId, location.tabId, location.paneId)
        // Opening a session means looking at a terminal, so Editor mode ends.
        return { activeWorkspaceId: location.workspaceId, selectedId: sessionId, activeView: 'terminal', workspaces }
      }
      const projectId = projectIdForWorkspace(requestedWorkspaceId)
      const projectWasRemoved = projectId !== null && state.removedProjectIds.includes(projectId)
      const workspaceId = projectWasRemoved ? UNASSIGNED_WORKSPACE : requestedWorkspaceId
      const current = state.workspaces[workspaceId] ?? emptyWorkspace()
      const activeTab: TerminalTab = {
        id: `tab-${sessionId}`,
        title,
        layout: pane(sessionId),
        activePaneId: `pane-${sessionId}`,
        closing: false,
      }
      const workspace = {
        ...current,
        tabs: [...current.tabs, activeTab],
        activeTabId: activeTab.id,
        defaultPending: false,
        error: null,
      }
      const workspaces = { ...state.workspaces, [workspaceId]: workspace }
      return activate || (projectWasRemoved && state.activeWorkspaceId === null)
        ? { activeWorkspaceId: workspaceId, selectedId: sessionId, activeView: 'terminal', workspaces }
        : { workspaces }
    })
    if (willActivate) get().requestEditorNavigation(open)
    else open()
  },
  setActiveTab: (workspaceId, tabId) =>
    set((state) => {
      const workspace = state.workspaces[workspaceId]
      const tab = workspace?.tabs.find((item) => item.id === tabId)
      if (!workspace || !tab) return state
      const workspaces = setActive(state.workspaces, workspaceId, tabId)
      return {
        activeWorkspaceId: workspaceId,
        selectedId: collectPanes(tab.layout).find((item) => item.id === tab.activePaneId)?.sessionId ?? null,
        workspaces,
      }
    }),
  focusPane: (workspaceId, tabId, paneId) =>
    set((state) => {
      const tab = state.workspaces[workspaceId]?.tabs.find((item) => item.id === tabId)
      const focused = tab && collectPanes(tab.layout).find((item) => item.id === paneId)
      if (!tab || !focused) return state
      return {
        activeWorkspaceId: workspaceId,
        selectedId: focused.sessionId,
        workspaces: setActive(state.workspaces, workspaceId, tabId, paneId),
      }
    }),
  splitPane: (workspaceId, tabId, paneId, direction, sessionId) => {
    let added = false
    set((state) => {
      if (sessionLocation(state.workspaces, sessionId)) return state
      const workspace = state.workspaces[workspaceId]
      const tab = workspace?.tabs.find((item) => item.id === tabId)
      if (!workspace || !tab || tab.closing) return state
      let addedPaneId: string | null = null
      const tabs = workspace.tabs.map((item) => {
        if (item.id !== tabId) return item
        const [layout, newPaneId] = splitTree(item.layout, paneId, direction, sessionId)
        addedPaneId = newPaneId
        return newPaneId ? { ...item, layout, activePaneId: newPaneId } : item
      })
      if (!addedPaneId) return state
      added = true
      return {
        activeWorkspaceId: workspaceId,
        selectedId: sessionId,
        workspaces: {
          ...state.workspaces,
          [workspaceId]: { ...workspace, tabs },
        },
      }
    });
    return added
  },
  setSplitRatio: (workspaceId, tabId, splitId, ratio) =>
    set((state) => {
      const workspace = state.workspaces[workspaceId]
      const tab = workspace?.tabs.find((item) => item.id === tabId)
      if (!workspace || !tab) return state
      const updateRatio = (node: PaneLayout): PaneLayout => {
        if (node.kind === 'pane') return node
        if (node.id === splitId) return { ...node, ratio: Math.min(0.95, Math.max(0.05, ratio)) }
        const first = updateRatio(node.first)
        const second = updateRatio(node.second)
        return first === node.first && second === node.second ? node : { ...node, first, second }
      }
      const tabs = workspace.tabs.map((item) => item.id === tabId ? { ...item, layout: updateRatio(item.layout) } : item)
      return { workspaces: { ...state.workspaces, [workspaceId]: { ...workspace, tabs } } }
    }),
  beginClosePane: (workspaceId, tabId, paneId, at) =>
    set((state) => {
      const workspace = state.workspaces[workspaceId]
      if (!workspace) return state
      const tabs = workspace.tabs.map((tab) => {
        if (tab.id !== tabId) return tab
        const [layout] = updateTree(tab.layout, paneId, (item) => ({
          ...item,
          closingAt: item.closingAt ?? at,
          stopRequestedAt: null,
          closeError: null,
        }))
        return { ...tab, layout, closing: collectPanes(layout).length === 1 && tab.layout.kind === 'pane' }
      })
      return { workspaces: { ...state.workspaces, [workspaceId]: { ...workspace, tabs } } }
    }),
  beginCloseTab: (workspaceId, tabId, at) =>
    set((state) => {
      const workspace = state.workspaces[workspaceId]
      if (!workspace) return state
      const tabs = workspace.tabs.map((tab) => {
        if (tab.id !== tabId) return tab
        const mark = (node: PaneLayout): PaneLayout => node.kind === 'pane'
          ? { ...node, closingAt: node.closingAt ?? at, closeError: null }
          : { ...node, first: mark(node.first), second: mark(node.second) }
        return { ...tab, layout: mark(tab.layout), closing: true }
      })
      return { workspaces: { ...state.workspaces, [workspaceId]: { ...workspace, tabs } }
      }
    }),
  markStopRequested: (sessionId, at) =>
    set((state) => ({
      workspaces: updateSession(state.workspaces, sessionId, (item) =>
        item.stopRequestedAt === null ? { ...item, stopRequestedAt: at } : item,
      ),
    })),
  retryClose: (sessionId) =>
    set((state) => ({
      workspaces: updateSession(state.workspaces, sessionId, (item) =>
        item.stopRequestedAt === null && item.closeError === null
          ? item
          : { ...item, stopRequestedAt: null, closeError: null },
      ),
    })),
  setCloseError: (sessionId, error) =>
    set((state) => ({
      workspaces: updateSession(state.workspaces, sessionId, (item) =>
        item.closeError === error ? item : { ...item, closeError: error },
      ),
    })),
  removeSession: (sessionId) =>
    set((state) => {
      const workspaces: Record<string, TerminalWorkspaceState> = {}
      for (const [workspaceId, workspace] of Object.entries(state.workspaces)) {
        const tabs = workspace.tabs.flatMap((tab) => {
          const layout = removePane(tab.layout, sessionId)
          if (!layout) return []
          return [{
            ...tab,
            layout,
            activePaneId: collectPanes(layout).some((item) => item.id === tab.activePaneId)
              ? tab.activePaneId
              : collectPanes(layout)[0].id,
            closing: tab.closing && collectPanes(layout).every((item) => item.closingAt !== null),
          }]
        })
        const activeTabId = tabs.some((tab) => tab.id === workspace.activeTabId)
          ? workspace.activeTabId
          : tabs[0]?.id ?? null
        const removedHere = workspace.tabs.some((tab) => collectPanes(tab.layout).some((item) => item.sessionId === sessionId))
        workspaces[workspaceId] = {
          ...workspace,
          tabs,
          activeTabId,
          defaultPending: removedHere ? false : workspace.defaultPending,
        }
      }
      const active = state.activeWorkspaceId ? workspaces[state.activeWorkspaceId] : undefined
      return { workspaces, selectedId: active ? focusedSession(active) : null }
    }),
  removeWorkspace: (workspaceId) =>
    set((state) => {
      if (!state.workspaces[workspaceId]) return state
      const workspaces = { ...state.workspaces }
      delete workspaces[workspaceId]
      const activeWorkspaceId = state.activeWorkspaceId === workspaceId ? null : state.activeWorkspaceId
      const active = activeWorkspaceId ? workspaces[activeWorkspaceId] : undefined
      return { workspaces, activeWorkspaceId, selectedId: active ? focusedSession(active) : null }
    }),
  removeProject: (projectId) =>
    set((state) => {
      const remaining = { ...state.workspaces }
      const removedKeys = Object.keys(remaining).filter((key) =>
        key === projectId || key.startsWith(`${projectId}${CHECKOUT_KEY_SEPARATOR}`),
      )
      const movedTabs = removedKeys.flatMap((key) => remaining[key].tabs)
      for (const key of removedKeys) delete remaining[key]
      const destination = remaining[UNASSIGNED_WORKSPACE] ?? emptyWorkspace()
      const unassigned = {
        ...destination,
        tabs: [...destination.tabs, ...movedTabs],
        activeTabId: destination.activeTabId ?? movedTabs[0]?.id ?? null,
      }
      remaining[UNASSIGNED_WORKSPACE] = unassigned
      const activeWorkspaceId = state.activeWorkspaceId !== null && removedKeys.includes(state.activeWorkspaceId)
        ? (unassigned.tabs.length > 0 ? UNASSIGNED_WORKSPACE : null)
        : state.activeWorkspaceId
      return {
        activeWorkspaceId,
        selectedId: activeWorkspaceId ? focusedSession(remaining[activeWorkspaceId]) : null,
        workspaces: remaining,
        removedProjectIds: [...new Set([...state.removedProjectIds, projectId])],
      }
    }),
  reset: () => set({ ...initialState }),
}))

/** The selected id remains valid only while its daemon session is listed. */
export function useSelectedId(): string | null {
  const id = useUi((state) => state.selectedId)
  const { data } = useSessionList()
  return id !== null && data?.some((session) => session.id === id) ? id : null
}
