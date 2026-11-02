import { memo } from 'react'
import { renderMarkdown } from './markdown-render'
import './markdown.css'

/**
 * Rendered model prose. The HTML is produced by `markdown-render` (marked +
 * DOMPurify), so `dangerouslySetInnerHTML` here carries sanitized markup only.
 */
export const Markdown = memo(function Markdown({ text, className }: { text: string; className?: string }): JSX.Element {
  return <div className={className ? `ark-md ${className}` : 'ark-md'} dangerouslySetInnerHTML={{ __html: renderMarkdown(text) }} />
})
