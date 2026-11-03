import { render, screen } from '@testing-library/react'
import { MarkdownView, isMarkdownPath } from './MarkdownView'

describe('isMarkdownPath', () => {
  it('accepts markdown extensions regardless of case', () => {
    expect(isMarkdownPath('README.md')).toBe(true)
    expect(isMarkdownPath('docs/GUIDE.MD')).toBe(true)
    expect(isMarkdownPath('notes.markdown')).toBe(true)
    expect(isMarkdownPath('page.mdx')).toBe(true)
  })

  it('rejects other files and missing paths', () => {
    expect(isMarkdownPath('src/app.ts')).toBe(false)
    expect(isMarkdownPath('README.md.txt')).toBe(false)
    expect(isMarkdownPath('md')).toBe(false)
    expect(isMarkdownPath(null)).toBe(false)
    expect(isMarkdownPath(undefined)).toBe(false)
  })
})

describe('MarkdownView', () => {
  it('renders headings, lists, tables, and fenced code', () => {
    render(
      <MarkdownView
        text={[
          '# Title',
          '',
          'Some **bold** text.',
          '',
          '- one',
          '- two',
          '',
          '| a | b |',
          '| - | - |',
          '| 1 | 2 |',
          '',
          '```ts',
          'const x = 1',
          '```',
        ].join('\n')}
      />,
    )

    expect(screen.getByRole('heading', { level: 1, name: 'Title' })).toBeTruthy()
    expect(screen.getByText('bold').tagName).toBe('STRONG')
    expect(screen.getAllByRole('listitem')).toHaveLength(2)
    expect(screen.getByRole('table')).toBeTruthy()
    expect(screen.getByText('const x = 1').tagName).toBe('CODE')
  })

  it('renders links as inert text with the href as a tooltip', () => {
    render(<MarkdownView text={'Read the [docs](https://example.com/guide).'} />)

    const link = screen.getByText('docs')
    expect(link.tagName).toBe('SPAN')
    expect(link.getAttribute('title')).toBe('https://example.com/guide')
    expect(screen.queryByRole('link')).toBeNull()
  })

  it('replaces images with a text placeholder', () => {
    render(<MarkdownView text={'![diagram](https://example.com/a.png)'} />)

    expect(screen.queryByRole('img')).toBeNull()
    expect(screen.getByText('[image: diagram]')).toBeTruthy()
  })

  it('drops raw HTML and keeps the rest of the document', () => {
    render(<MarkdownView text={'<div class="x">raw</div>\n\n# Kept'} />)

    expect(screen.queryByText('raw')).toBeNull()
    expect(screen.getByRole('heading', { level: 1, name: 'Kept' })).toBeTruthy()
  })
})
