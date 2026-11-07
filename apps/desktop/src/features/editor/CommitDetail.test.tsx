import { act, fireEvent, render, screen, within } from '@testing-library/react'
import type { GitViewCommit, GitViewCommitDetail, GitViewDiff } from '../../services/types'
import { useUi } from '../../stores/ui'
import { CommitDetail, UncommittedDetail } from './CommitDetail'
import { formatTime } from './panes'

const COMMIT_ID = '0123456789abcdef0123456789abcdef01234567'
const TIME = 1_760_000_000_000

const commit = (over: Partial<GitViewCommit> = {}): GitViewCommit => ({
  id: COMMIT_ID,
  parents: [],
  refs: [],
  head: false,
  author: 'Ada Lovelace',
  time: TIME,
  subject: 'Add the thing',
  ...over,
})

const detail = (over: Partial<GitViewCommitDetail> = {}): GitViewCommitDetail => ({
  author_email: 'ada@example.com',
  committer: 'Ada Lovelace',
  committer_email: 'ada@example.com',
  body: '',
  ...over,
})

const diff = (over: Partial<GitViewDiff> = {}): GitViewDiff => ({
  label: 'commit 01234567',
  patch: ['--- a/f.txt', '+++ b/f.txt', '@@ -1 +1 @@', '-old', '+new'].join('\n'),
  truncated: false,
  files: [],
  ...over,
})

const setClipboard = (value: unknown) => {
  Object.defineProperty(navigator, 'clipboard', { value, configurable: true, writable: true })
}

beforeEach(() => {
  useUi.getState().reset()
  setClipboard(undefined)
})

afterEach(() => {
  vi.useRealTimers()
  setClipboard(undefined)
})

describe('CommitDetail', () => {
  it('shows body, author email and date', () => {
    render(
      <CommitDetail
        commit={commit({ head: true, refs: ['main', 'remotes/origin/main', 'tag:v1.0'] })}
        diff={diff({ detail: detail({ body: 'First body line\nsecond body line' }) })}
        stale={null}
        loadedIds={new Set()}
        onSelectParent={() => {}}
      />,
    )
    expect(screen.getByText('Add the thing')).toBeTruthy()
    expect(screen.getByText('01234567')).toBeTruthy()
    expect(screen.getByText('HEAD')).toBeTruthy()
    expect(screen.getByText('main')).toBeTruthy()
    expect(screen.getByText('origin/main')).toBeTruthy()
    expect(screen.getByText('v1.0')).toBeTruthy()
    const author = screen.getByText(/Ada Lovelace/)
    expect(author.textContent).toContain('<ada@example.com>')
    expect(author.textContent).toContain(formatTime(TIME))
    expect(screen.getByText(/First body line/).textContent).toContain('second body line')
    // A two-line body needs no expander and a non-merge commit no first-parent note.
    expect(screen.queryByRole('button', { name: 'Show more' })).toBeNull()
    expect(screen.queryByText('Changes compared with the first parent')).toBeNull()
  })

  it('committer line only when different', () => {
    const view = (over: Partial<GitViewCommitDetail> | null) => (
      <CommitDetail
        commit={commit()}
        diff={diff(over === null ? {} : { detail: detail(over) })}
        stale={null}
        loadedIds={new Set()}
        onSelectParent={() => {}}
      />
    )
    const { rerender } = render(view({}))
    expect(screen.queryByText(/Committer:/)).toBeNull()

    rerender(view({ committer: 'Grace Hopper', committer_email: 'grace@example.com' }))
    expect(screen.getByText(/Committer: Grace Hopper <grace@example\.com>/)).toBeTruthy()

    // Same name but a different email still counts as different.
    rerender(view({ committer_email: 'ada@work.example.com' }))
    expect(screen.getByText(/Committer: Ada Lovelace <ada@work\.example\.com>/)).toBeTruthy()

    rerender(view(null))
    expect(screen.queryByText(/Committer:/)).toBeNull()
  })

  it('parent button selects a loaded parent and is disabled otherwise', () => {
    const loaded = 'aaaabbbbccccddddeeeeffff0000111122223333'
    const missing = 'ffff0000eeee1111dddd2222cccc3333bbbb4444'
    const onSelectParent = vi.fn()
    render(
      <CommitDetail
        commit={commit({ parents: [loaded, missing] })}
        diff={diff()}
        stale={null}
        loadedIds={new Set([loaded])}
        onSelectParent={onSelectParent}
      />,
    )
    const enabled = screen.getByRole('button', { name: `Select parent ${loaded.slice(0, 8)}` })
    const disabled = screen.getByRole('button', { name: `Select parent ${missing.slice(0, 8)}` })
    expect((enabled as HTMLButtonElement).disabled).toBe(false)
    expect((disabled as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(enabled)
    expect(onSelectParent).toHaveBeenCalledWith(loaded)
    expect(onSelectParent).toHaveBeenCalledTimes(1)
  })

  it('long body collapses and expands', () => {
    const body = Array.from({ length: 8 }, (_, index) => `body line ${index + 1}`).join('\n')
    render(
      <CommitDetail
        commit={commit()}
        diff={diff({ detail: detail({ body }) })}
        stale={null}
        loadedIds={new Set()}
        onSelectParent={() => {}}
      />,
    )
    expect(screen.getByText(/body line 1/).className).toContain('line-clamp-6')
    fireEvent.click(screen.getByRole('button', { name: 'Show more' }))
    expect(screen.getByRole('button', { name: 'Show less' })).toBeTruthy()
    expect(screen.getByText(/body line 1/).className).not.toContain('line-clamp-6')
  })

  it('merge commit shows the first-parent note', () => {
    render(
      <CommitDetail
        commit={commit({ parents: ['parent-one', 'parent-two'] })}
        diff={diff()}
        stale={null}
        loadedIds={new Set()}
        onSelectParent={() => {}}
      />,
    )
    expect(screen.getByText('Changes compared with the first parent')).toBeTruthy()
  })

  it('files show +added −removed and binary shows none', () => {
    render(
      <CommitDetail
        commit={commit()}
        diff={diff({
          files: [
            { path: 'src/a.ts', orig_path: null, status: 'modified', added: 12, removed: 3 },
            { path: 'logo.png', orig_path: null, status: 'modified', added: null, removed: null },
            { path: 'gone.txt', orig_path: null, status: 'deleted' },
          ],
        })}
        stale={null}
        loadedIds={new Set()}
        onSelectParent={() => {}}
      />,
    )
    const list = screen.getByRole('list', { name: 'Changed files' })
    const textFile = within(list).getByText('src/a.ts').closest('li')!
    expect(within(textFile).getByText('+12')).toBeTruthy()
    expect(within(textFile).getByText('−3')).toBeTruthy()
    for (const name of ['logo.png', 'gone.txt']) {
      const item = within(list).getByText(name).closest('li')!
      expect(within(item).queryByText(/\+\d/)).toBeNull()
      expect(within(item).queryByText(/−\d/)).toBeNull()
    }
  })

  it('copy button writes the full oid and shows Copied', async () => {
    vi.useFakeTimers()
    const writeText = vi.fn().mockResolvedValue(undefined)
    setClipboard({ writeText })
    render(
      <CommitDetail
        commit={commit()}
        diff={diff()}
        stale={null}
        loadedIds={new Set()}
        onSelectParent={() => {}}
      />,
    )
    fireEvent.click(screen.getByRole('button', { name: 'Copy hash' }))
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(writeText).toHaveBeenCalledWith(COMMIT_ID)
    expect(screen.getByText('Copied')).toBeTruthy()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1600)
    })
    expect(screen.queryByText('Copied')).toBeNull()
  })

  it('copy rejection shows Copy failed', async () => {
    vi.useFakeTimers()
    const writeText = vi.fn().mockRejectedValue(new Error('denied'))
    setClipboard({ writeText })
    render(
      <CommitDetail
        commit={commit()}
        diff={diff()}
        stale={null}
        loadedIds={new Set()}
        onSelectParent={() => {}}
      />,
    )
    fireEvent.click(screen.getByRole('button', { name: 'Copy hash' }))
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(screen.getByText('Copy failed')).toBeTruthy()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1600)
    })
    expect(screen.queryByText('Copy failed')).toBeNull()

    // A missing clipboard API fails the same way.
    setClipboard(undefined)
    fireEvent.click(screen.getByRole('button', { name: 'Copy hash' }))
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(screen.getByText('Copy failed')).toBeTruthy()
  })

  it('truncated patch keeps its notice', () => {
    render(
      <CommitDetail
        commit={commit()}
        diff={diff({ truncated: true })}
        stale={null}
        loadedIds={new Set()}
        onSelectParent={() => {}}
      />,
    )
    expect(screen.getByText(/larger than 1 MiB/)).toBeTruthy()
    expect(screen.getByText(/a\/f\.txt/)).toBeTruthy()
  })
})

describe('UncommittedDetail', () => {
  it('lists entries and Open in Changes calls back', () => {
    const onOpenChanges = vi.fn()
    render(
      <UncommittedDetail
        entries={[
          { path: 'src/a.ts', orig_path: null, status: 'modified' },
          { path: 'new.ts', orig_path: 'old.ts', status: 'renamed' },
          { path: 'notes.txt', orig_path: null, status: 'untracked' },
        ]}
        onOpenChanges={onOpenChanges}
      />,
    )
    expect(screen.getByText('Uncommitted changes')).toBeTruthy()
    expect(screen.getByText('src/a.ts')).toBeTruthy()
    expect(screen.getByText('old.ts → new.ts')).toBeTruthy()
    expect(screen.getByText('modified')).toBeTruthy()
    expect(screen.getByText('renamed')).toBeTruthy()
    expect(screen.getByText('untracked')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Open in Changes' }))
    expect(onOpenChanges).toHaveBeenCalledTimes(1)
  })
})
