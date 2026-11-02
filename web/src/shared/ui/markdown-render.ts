/**
 * Markdown -> sanitized HTML, the single render path for model-authored prose.
 *
 * Assistant output is untrusted input: everything passes through DOMPurify
 * before it ever reaches `dangerouslySetInnerHTML`. Fenced blocks are
 * highlighted with highlight.js; the emitted `hljs-*` spans are plain
 * `<span class>` markup that DOMPurify keeps.
 */
import { marked } from 'marked'
import DOMPurify from 'dompurify'
import hljs from 'highlight.js/lib/common'

/** Largest source that still gets highlight.js; beyond this, escape and bail. */
const MAX_HIGHLIGHT_CHARS = 100_000

const renderer = {
  code({ text, lang }: { text: string; lang?: string }): string {
    const language = (lang ?? '').trim().split(/\s+/)[0]
    const body = text.length > MAX_HIGHLIGHT_CHARS
      ? escapeHtml(text)
      : highlight(text, language)
    const langClass = language ? ` language-${language}` : ''
    return `<pre><code class="hljs"${langClass}>${body}</code></pre>`
  }
}

marked.use({ gfm: true, breaks: false, async: false, renderer })

function highlight(text: string, language: string | undefined): string {
  try {
    if (language && hljs.getLanguage(language)) {
      return hljs.highlight(text, { language, ignoreIllegals: true }).value
    }
    return hljs.highlightAuto(text).value
  } catch {
    // A highlighter failure must never take the message row down with it.
    return escapeHtml(text)
  }
}

export function escapeHtml(text: string): string {
  return text
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;')
}

/** Parse, highlight and sanitize in one step; always synchronous. */
export function renderMarkdown(text: string): string {
  // `async: false` keeps the return type a plain string: the renderer is a
  // synchronous projection step, never an async pipeline.
  const html = marked.parse(text, { async: false })
  return DOMPurify.sanitize(html)
}
