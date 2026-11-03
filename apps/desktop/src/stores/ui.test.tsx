import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { makeQueryClient, queryKeys } from '../queryClient'
import type { SessionInfo } from '../services/types'
import { checkoutForCwd, checkoutKey, useSelectedId, useUi } from './ui'

const { session } = vi.hoisted(() => ({ session: { list: vi.fn() } }))
vi.mock('../services/session', () => ({ sessionService: session }))

const makeSession = (id: string): SessionInfo => ({
  id,
  title: id,
  argv: ['sh'],
  cwd: '/tmp',
  state: 'running',
  exit_code: null,
  error: null,
  started_at: 1,
  ended_at: null,
})

let qc: QueryClient
const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>

beforeEach(() => {
  qc = makeQueryClient()
  qc.setQueryData(queryKeys.sessions, [makeSession('a'), makeSession('b')])
  useUi.getState().reset()
})

describe('useSelectedId', () => {
  it('returns the selected id while that session exists', async () => {
    const { result } = renderHook(() => useSelectedId(), { wrapper })
    expect(result.current).toBeNull()
    act(() => useUi.getState().select('a'))
    expect(result.current).toBe('a')
  })

  it('returns null once the list loses the session, without calling select', async () => {
    const { result } = renderHook(() => useSelectedId(), { wrapper })
    act(() => useUi.getState().select('a'))
    expect(result.current).toBe('a')

    act(() => void qc.setQueryData(queryKeys.sessions, [makeSession('b')]))
    await waitFor(() => expect(result.current).toBeNull())
    expect(useUi.getState().selectedId).toBe('a')
  })

  it('returns null for an unknown id', () => {
    const { result } = renderHook(() => useSelectedId(), { wrapper })
    act(() => useUi.getState().select('nope'))
    expect(result.current).toBeNull()
  })
})

describe('useUi', () => {
  it('stores project-keyed tabs and focuses an existing session instead of duplicating it', () => {
    act(() => {
      useUi.getState().activateWorkspace('p1')
      useUi.getState().openSession('p1', 'a', 'Shell')
    })
    const firstTab = useUi.getState().workspaces.p1.tabs[0].id

    act(() => useUi.getState().openSession('p1', 'a', 'Shell'))

    expect(useUi.getState().workspaces.p1.tabs).toHaveLength(1)
    expect(useUi.getState().workspaces.p1.activeTabId).toBe(firstTab)
    expect(useUi.getState().selectedId).toBe('a')
  })

  it('supports nested splits and collapses a deleted pane', () => {
    act(() => {
      useUi.getState().activateWorkspace('p1')
      useUi.getState().openSession('p1', 'a', 'Shell')
      useUi.getState().splitPane('p1', 'tab-a', 'pane-a', 'vertical', 'b')
      useUi.getState().splitPane('p1', 'tab-a', 'pane-b', 'horizontal', 'c')
      useUi.getState().setSplitRatio('p1', 'tab-a', 'split-pane-c', 0.7)
    })

    const tab = useUi.getState().workspaces.p1.tabs[0]
    expect(tab.layout.kind).toBe('split')
    if (tab.layout.kind !== 'split') throw new Error('expected a split')
    expect(tab.layout.direction).toBe('vertical')
    expect(tab.layout.second.kind).toBe('split')
    if (tab.layout.second.kind !== 'split') throw new Error('expected a nested split')
    expect(tab.layout.second.direction).toBe('horizontal')
    expect(tab.layout.second.ratio).toBe(0.7)

    act(() => useUi.getState().removeSession('c'))
    const collapsed = useUi.getState().workspaces.p1.tabs[0].layout
    expect(collapsed.kind).toBe('split')
    if (collapsed.kind !== 'split') throw new Error('expected the parent split to remain')
    expect(collapsed.second).toMatchObject({ kind: 'pane', sessionId: 'b' })
  })

  it('moves an open project workspace to Unassigned without touching its sessions', () => {
    act(() => {
      useUi.getState().activateWorkspace('p1')
      useUi.getState().openSession('p1', 'a', 'Shell')
      useUi.getState().removeProject('p1')
    })

    expect(useUi.getState().activeWorkspaceId).toBe('__unassigned__')
    expect(useUi.getState().workspaces.p1).toBeUndefined()
    expect(useUi.getState().workspaces.__unassigned__.tabs[0].layout).toMatchObject({
      kind: 'pane',
      sessionId: 'a',
    })
  })

  it('shows a default session that finishes launching after its project is removed', () => {
    act(() => {
      const checkout = checkoutKey('p1', '/work/p1')
      useUi.getState().activateWorkspace(checkout)
      useUi.getState().claimDefaultTab(checkout)
      useUi.getState().removeProject('p1')
      useUi.getState().openSession(checkout, 'late', 'Shell', false)
    })

    expect(useUi.getState().activeWorkspaceId).toBe('__unassigned__')
    expect(useUi.getState().workspaces.__unassigned__.tabs[0].layout).toMatchObject({
      kind: 'pane',
      sessionId: 'late',
    })
  })

  it('restores each project tab selection after switching workspaces', () => {
    act(() => {
      useUi.getState().activateWorkspace('p1')
      useUi.getState().openSession('p1', 'a', 'Shell A')
      useUi.getState().activateWorkspace('p2')
      useUi.getState().openSession('p2', 'b', 'Shell B')
      useUi.getState().activateWorkspace('p1')
    })

    expect(useUi.getState().activeWorkspaceId).toBe('p1')
    expect(useUi.getState().workspaces.p1.tabs.map((tab) => tab.title)).toEqual(['Shell A'])
    expect(useUi.getState().workspaces.p1.activeTabId).toBe('tab-a')
    expect(useUi.getState().workspaces.p2.tabs.map((tab) => tab.title)).toEqual(['Shell B'])
  })

  it('isolates tabs by checkout and rehomes every checkout when its project is removed', () => {
    const root = checkoutKey('p1', '/work/p1')
    const feature = checkoutKey('p1', '/work/p1-feature')
    act(() => {
      useUi.getState().activateWorkspace(root)
      useUi.getState().openSession(root, 'root-session', 'Root Shell')
      useUi.getState().activateWorkspace(feature)
      useUi.getState().openSession(feature, 'feature-session', 'Feature Shell')
      useUi.getState().removeProject('p1')
    })

    expect(useUi.getState().workspaces[root]).toBeUndefined()
    expect(useUi.getState().workspaces[feature]).toBeUndefined()
    expect(useUi.getState().workspaces.__unassigned__.tabs.map((tab) => tab.title)).toEqual(['Root Shell', 'Feature Shell'])
    expect(useUi.getState().activeWorkspaceId).toBe('__unassigned__')
  })

  it('keeps the Editor view across checkout switches but returns to Terminal on session open', () => {
    const root = checkoutKey('p1', '/work/p1')
    const feature = checkoutKey('p1', '/work/p1-feature')
    act(() => {
      useUi.getState().activateWorkspace(root)
      useUi.getState().setActiveView('editor')
      useUi.getState().activateWorkspace(feature)
    })
    expect(useUi.getState().activeView).toBe('editor')

    act(() => useUi.getState().openSession(feature, 'a', 'Shell'))
    expect(useUi.getState().activeView).toBe('terminal')
  })
})

describe('checkoutForCwd', () => {
  it('assigns exact and descendant working directories to the most specific checkout', () => {
    const checkouts = [{ path: '/work/project' }, { path: '/work/project/nested' }]
    expect(checkoutForCwd(checkouts, '/work/project')).toBe('/work/project')
    expect(checkoutForCwd(checkouts, '/work/project/src')).toBe('/work/project')
    expect(checkoutForCwd(checkouts, '/work/project/nested/src/')).toBe('/work/project/nested')
    expect(checkoutForCwd(checkouts, '/work/project-other')).toBeNull()
  })
})
