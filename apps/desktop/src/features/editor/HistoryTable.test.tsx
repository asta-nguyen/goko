import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { createRef, type ComponentProps, type Ref } from 'react'
import type { GitViewCommit } from '../../services/types'
import { GRAPH_PALETTE } from './history-graph'
import { formatRelative, HistoryTable, type HistoryTableHandle } from './HistoryTable'
import { useHistoryLayout } from './history-layout'
import { formatTime } from './panes'

const commit = (id: string, over: Partial<GitViewCommit> = {}): GitViewCommit => ({
  id,
  parents: [],
  refs: [],
  head: false,
  author: 'Ada Lovelace',
  time: Date.now() - 5 * 60_000,
  subject: `subject ${id}`,
  ...over,
})

const COMMITS = [
  commit('a1b2c3d4e5f6', { head: true, refs: ['main'], parents: ['b2c3d4e5f6a7'], subject: 'Add the graph' }),
  commit('b2c3d4e5f6a7', { parents: ['c3d4e5f6a7b8'], subject: 'Wire the table' }),
  commit('c3d4e5f6a7b8', { subject: 'Ship it' }),
]

type TableProps = Omit<ComponentProps<typeof HistoryTable>, 'layout' | 'onColumn' | 'onResetColumn' | 'ref'>

const defaults = (over: Partial<TableProps> = {}): TableProps => ({
  commits: COMMITS,
  selected: null,
  onSelect: () => {},
  matches: null,
  hasNextPage: false,
  isFetchingNextPage: false,
  onLoadMore: () => {},
  uncommitted: null,
  onSelectUncommitted: () => {},
  ...over,
})

// The container owns useHistoryLayout; the test harness plays that role so
// column drags exercise the real persistence path.
const Harness = ({ tableRef, ...props }: TableProps & { tableRef?: Ref<HistoryTableHandle> }) => {
  const { layout, setColumn, reset } = useHistoryLayout()
  return <HistoryTable ref={tableRef} layout={layout} onColumn={setColumn} onResetColumn={reset} {...props} />
}

const renderTable = (initial: Partial<TableProps> = {}, tableRef?: Ref<HistoryTableHandle>) => {
  let over = initial
  const view = render(<Harness tableRef={tableRef} {...defaults(over)} />)
  return {
    ...view,
    update: (next: Partial<TableProps>) => {
      over = { ...over, ...next }
      view.rerender(<Harness tableRef={tableRef} {...defaults(over)} />)
    },
  }
}

class FakeIntersectionObserver {
  static instances: FakeIntersectionObserver[] = []
  elements: Element[] = []
  private callback: (entries: { isIntersecting: boolean }[], observer: unknown) => void
  constructor(callback: (entries: { isIntersecting: boolean }[], observer: unknown) => void) {
    this.callback = callback
    FakeIntersectionObserver.instances.push(this)
  }
  observe(element: Element) {
    this.elements.push(element)
  }
  unobserve() {}
  disconnect() {}
  fire(isIntersecting = true) {
    this.callback([{ isIntersecting }], this)
  }
}

const writeText = vi.fn()

beforeEach(() => {
  localStorage.clear()
  FakeIntersectionObserver.instances = []
  writeText.mockReset().mockResolvedValue(undefined)
  Object.defineProperty(window.navigator, 'clipboard', {
    value: { writeText },
    configurable: true,
  })
})

afterEach(() => vi.unstubAllGlobals())

describe('HistoryTable', () => {
  it('renders one row per commit with short hash, relative date and title', () => {
    const { container } = renderTable()
    expect(container.querySelectorAll('[data-nav-item]')).toHaveLength(3)

    const row = screen.getByRole('button', { name: /Add the graph/ })
    expect(within(row).getByText('Add the graph')).toBeTruthy()
    expect(within(row).getByText('a1b2c3d4')).toBeTruthy()
    const date = within(row).getByText('5 minutes ago')
    expect(date.getAttribute('title')).toBe(formatTime(COMMITS[0].time))
  })

  it('HEAD row has the HEAD chip and ref chips', () => {
    renderTable({
      commits: [commit('a1b2c3d4e5f6', { head: true, refs: ['main', 'remotes/origin/x', 'tag:v1.0'] })],
    })
    const row = screen.getByRole('button', { name: /subject a1b2c3d4e5f6/ })
    expect(within(row).getByText('HEAD')).toBeTruthy()
    expect(within(row).getByText('main')).toBeTruthy()
    expect(within(row).getByText('origin/x')).toBeTruthy()
    expect(within(row).getByText('v1.0')).toBeTruthy()

    // The chip is the outer badge; its title carries the full ref name.
    const chip = within(row).getByTitle('main')
    // jsdom serializes hsl() to rgb(); compare against the normalized palette colour.
    const probe = document.createElement('span')
    probe.style.color = GRAPH_PALETTE[0]
    expect(chip.style.color).toBe(probe.style.color)
    expect(chip.style.borderColor).toBe(probe.style.color)
  })

  it('filter dims non-matches, keeps all rows and the graph, and shows N matches', () => {
    const { container } = renderTable({ matches: new Set([COMMITS[0].id, COMMITS[2].id]) })
    expect(container.querySelectorAll('[data-nav-item]')).toHaveLength(3)
    expect(screen.getByRole('button', { name: /Wire the table/ }).className).toContain('opacity-40')
    expect(screen.getByRole('button', { name: /Add the graph/ }).className).not.toContain('opacity-40')
    expect(container.querySelector('svg[aria-hidden="true"]')).toBeTruthy()
    expect(screen.getByText('2 matches')).toBeTruthy()
  })

  it('zero matches reads 0 matches', () => {
    const { container } = renderTable({ matches: new Set() })
    expect(screen.getByText('0 matches')).toBeTruthy()
    for (const item of container.querySelectorAll('[data-nav-item]')) {
      expect(item.className).toContain('opacity-40')
    }
  })

  it('ArrowDown/ArrowUp/Home/End move focus but do not select', () => {
    const onSelect = vi.fn()
    const { container } = renderTable({ onSelect })
    const items = Array.from(container.querySelectorAll<HTMLElement>('[data-nav-item]'))
    expect(items).toHaveLength(3)

    const press = (key: string) => fireEvent.keyDown(document.activeElement as Element, { key })
    items[0].focus()
    press('ArrowDown')
    expect(document.activeElement).toBe(items[1])
    press('ArrowDown')
    expect(document.activeElement).toBe(items[2])
    press('ArrowDown')
    expect(document.activeElement).toBe(items[2])
    press('ArrowUp')
    expect(document.activeElement).toBe(items[1])
    press('Home')
    expect(document.activeElement).toBe(items[0])
    press('End')
    expect(document.activeElement).toBe(items[2])
    expect(onSelect).not.toHaveBeenCalled()
  })

  it('Enter and Space select', () => {
    const onSelect = vi.fn()
    renderTable({ onSelect })
    const row = screen.getByRole('button', { name: /Add the graph/ })
    // Browsers turn Enter/Space on a button into a click; jsdom has no key
    // activation, so the click stands in for both keys.
    fireEvent.keyDown(row, { key: 'Enter' })
    fireEvent.click(row)
    fireEvent.keyDown(row, { key: ' ' })
    fireEvent.click(row)
    expect(onSelect).toHaveBeenCalledTimes(2)
    expect(onSelect).toHaveBeenCalledWith(COMMITS[0].id)
  })

  it('sentinel intersection calls onLoadMore once', () => {
    vi.stubGlobal('IntersectionObserver', FakeIntersectionObserver)
    const onLoadMore = vi.fn()
    const { update } = renderTable({ hasNextPage: true, onLoadMore })

    const observer = FakeIntersectionObserver.instances.at(-1)!
    expect(observer.elements).toHaveLength(1)
    observer.fire(true)
    expect(onLoadMore).toHaveBeenCalledTimes(1)

    update({ isFetchingNextPage: true })
    observer.fire(true)
    expect(onLoadMore).toHaveBeenCalledTimes(1)
  })

  it('observes the sentinel again after a page loads', () => {
    vi.stubGlobal('IntersectionObserver', FakeIntersectionObserver)
    const { update } = renderTable({ hasNextPage: true })
    const before = FakeIntersectionObserver.instances.length

    update({ commits: [...COMMITS, { ...COMMITS[0], id: 'extra000', parents: [] }] })

    // A sentinel that stays in view needs a fresh observer to report intersecting again.
    expect(FakeIntersectionObserver.instances.length).toBeGreaterThan(before)
  })

  it('Load more button works without IntersectionObserver', () => {
    vi.stubGlobal('IntersectionObserver', undefined)
    const onLoadMore = vi.fn()
    const { update } = renderTable({ hasNextPage: true, onLoadMore })

    fireEvent.click(screen.getByRole('button', { name: 'Load more commits' }))
    expect(onLoadMore).toHaveBeenCalledTimes(1)

    update({ isFetchingNextPage: true })
    const loading = screen.getByRole('button', { name: 'Loading…' }) as HTMLButtonElement
    expect(loading.disabled).toBe(true)
    fireEvent.click(loading)
    expect(onLoadMore).toHaveBeenCalledTimes(1)
  })

  it('uncommitted row renders only when provided and selects', () => {
    const onSelectUncommitted = vi.fn()
    const { container, update } = renderTable({ onSelectUncommitted })
    expect(screen.queryByText(/Uncommitted changes/)).toBeNull()
    expect(container.querySelectorAll('[data-nav-item]')).toHaveLength(3)

    update({ uncommitted: { count: 4, selected: false } })
    const row = screen.getByRole('button', { name: /Uncommitted changes \(4\)/ })
    expect(row.getAttribute('aria-pressed')).toBe('false')
    fireEvent.click(row)
    expect(onSelectUncommitted).toHaveBeenCalledTimes(1)
    expect(container.querySelectorAll('[data-nav-item]')).toHaveLength(4)

    update({ uncommitted: { count: 4, selected: true } })
    expect(screen.getByRole('button', { name: /Uncommitted changes/ }).getAttribute('aria-pressed')).toBe('true')
  })

  it('context menu copies full hash and subject', async () => {
    renderTable()
    const row = screen.getByRole('button', { name: /Add the graph/ })

    fireEvent.contextMenu(row)
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Copy hash' }))
    expect(writeText).toHaveBeenCalledWith(COMMITS[0].id)

    fireEvent.contextMenu(row)
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Copy subject' }))
    expect(writeText).toHaveBeenCalledWith('Add the graph')
    expect(writeText).toHaveBeenCalledTimes(2)
  })

  it('clipboard rejection does not throw', async () => {
    writeText.mockRejectedValue(new Error('denied'))
    renderTable()
    const row = screen.getByRole('button', { name: /Add the graph/ })

    fireEvent.contextMenu(row)
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Copy hash' }))
    await waitFor(() => expect(writeText).toHaveBeenCalled())
    expect(screen.getByRole('button', { name: /Add the graph/ })).toBeTruthy()
  })

  it('dragging the Author handle changes its width and stores it', () => {
    renderTable()
    const handle = screen.getByRole('separator', { name: 'Resize author column' })
    expect(handle.getAttribute('aria-valuenow')).toBe('128')

    fireEvent.pointerDown(handle, { pointerId: 1, clientX: 400, buttons: 1 })
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 380, buttons: 1 })
    fireEvent.pointerUp(handle, { pointerId: 1 })
    expect(handle.getAttribute('aria-valuenow')).toBe('148')

    const stored = JSON.parse(localStorage.getItem('goko.history.layout')!) as { author: number }
    expect(stored.author).toBe(148)
  })

  it('a narrow container keeps the table at its minimum width and scrolls', () => {
    const { container } = renderTable()
    const scroller = container.querySelector('.overflow-auto')
    expect(scroller).toBeTruthy()
    const inner = scroller!.firstElementChild as HTMLElement
    // One lane: the natural graph width clamps to 48, so 48 + 120 + 128 + 80 + 200.
    expect(inner.style.minWidth).toBe('576px')
  })

  it('exposes scrollTo through the ref', () => {
    const calls: unknown[] = []
    const proto = window.HTMLElement.prototype as {
      scrollIntoView?: (options?: ScrollIntoViewOptions) => void
    }
    const original = proto.scrollIntoView
    proto.scrollIntoView = (options) => {
      calls.push(options)
    }
    try {
      const ref = createRef<HistoryTableHandle>()
      renderTable({}, ref)
      ref.current!.scrollTo(COMMITS[1].id)
      ref.current!.scrollTo('missing-id')
      expect(calls).toEqual([{ block: 'nearest' }])

      // jsdom without scrollIntoView must not throw.
      delete proto.scrollIntoView
      expect(() => ref.current!.scrollTo(COMMITS[0].id)).not.toThrow()
    } finally {
      proto.scrollIntoView = original
    }
  })
})

describe('formatRelative', () => {
  const now = 1_800_000_000_000

  it('says just now under a minute', () => {
    expect(formatRelative(now - 59_000, now)).toBe('just now')
    expect(formatRelative(now + 5_000, now)).toBe('just now')
  })

  it('uses relative units up to 30 days', () => {
    expect(formatRelative(now - 5 * 60_000, now)).toBe('5 minutes ago')
    expect(formatRelative(now - 3 * 3_600_000, now)).toBe('3 hours ago')
    expect(formatRelative(now - 3 * 86_400_000, now)).toBe('3 days ago')
    expect(formatRelative(now - 86_400_000, now)).toBe('yesterday')
  })

  it('uses the medium date after 30 days', () => {
    const old = now - 45 * 86_400_000
    const medium = (ms: number) => new Date(ms).toLocaleDateString(undefined, { dateStyle: 'medium' })
    expect(formatRelative(old, now)).toBe(medium(old))
    expect(formatRelative(now - 30 * 86_400_000, now)).toBe(medium(now - 30 * 86_400_000))
  })
})
