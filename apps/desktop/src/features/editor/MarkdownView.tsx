import ReactMarkdown, { type Components } from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { cn } from '@/lib/utils'

/** File extensions that open with the rendered preview. */
export const isMarkdownPath = (path: string | null | undefined) =>
  path !== null && path !== undefined && /\.(md|markdown|mdx)$/i.test(path)

// Links and images stay inert: the app has no opener capability and no asset
// base path, so neither may navigate the webview or load a remote resource.
const components: Components = {
  a: ({ href, children }) => (
    <span className="text-accent underline underline-offset-2" title={href}>
      {children}
    </span>
  ),
  img: ({ alt, src }) => (
    <span
      className="inline-flex max-w-full items-center rounded border border-border bg-panel px-1.5 py-0.5 font-mono text-xs text-muted"
      title={src}
    >
      [image: {alt || src || 'untitled'}]
    </span>
  ),
}

const PROSE = cn(
  'min-w-0 max-w-3xl p-4 text-sm leading-6 text-text',
  '[&_h1]:mt-0 [&_h1]:mb-3 [&_h1]:text-xl [&_h1]:font-semibold',
  '[&_h2]:mt-5 [&_h2]:mb-2 [&_h2]:text-lg [&_h2]:font-semibold',
  '[&_h3]:mt-4 [&_h3]:mb-2 [&_h3]:text-base [&_h3]:font-semibold',
  '[&_p]:my-2',
  '[&_ul]:my-2 [&_ul]:list-disc [&_ul]:pl-5',
  '[&_ol]:my-2 [&_ol]:list-decimal [&_ol]:pl-5',
  '[&_li]:my-0.5',
  '[&_blockquote]:my-2 [&_blockquote]:border-l-2 [&_blockquote]:border-border [&_blockquote]:pl-3 [&_blockquote]:text-muted',
  '[&_code]:rounded [&_code]:bg-bg [&_code]:px-1 [&_code]:py-0.5 [&_code]:font-mono [&_code]:text-xs',
  '[&_pre]:my-2 [&_pre]:overflow-auto [&_pre]:rounded [&_pre]:border [&_pre]:border-border [&_pre]:bg-bg [&_pre]:p-3',
  '[&_pre>code]:bg-transparent [&_pre>code]:p-0',
  '[&_table]:my-3 [&_table]:w-full [&_table]:border-collapse [&_table]:text-xs',
  '[&_th]:border [&_th]:border-border [&_th]:bg-panel [&_th]:px-2 [&_th]:py-1 [&_th]:text-left',
  '[&_td]:border [&_td]:border-border [&_td]:px-2 [&_td]:py-1',
  '[&_hr]:my-4 [&_hr]:border-border',
)

/** Rendered, read-only Markdown. Raw HTML is dropped by react-markdown. */
export function MarkdownView({ text }: { text: string }) {
  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <article className={PROSE}>
        <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
          {text}
        </ReactMarkdown>
      </article>
    </div>
  )
}
