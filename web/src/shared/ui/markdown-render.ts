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
import markedCjkFriendly from 'marked-cjk-friendly'

/** Largest source that still gets highlight.js; beyond this, escape and bail. */
const MAX_HIGHLIGHT_CHARS = 100_000

const renderer = {
  code({ text, lang }: { text: string; lang?: string }): string {
    const language = (lang ?? '').trim().split(/\s+/)[0]
    const body = text.length > MAX_HIGHLIGHT_CHARS
      ? escapeHtml(text)
      : highlight(text, language)
    const langClass = language ? ` language-${language}` : ''
    // The block ships with its own header (language label + copy button); the
    // raw source rides in a `data-copy-source` attribute so the button works
    // without re-deriving text from the highlighted DOM.
    return (
      `<div class="ark-md-code">` +
      `<div class="ark-md-code__bar"><span class="ark-md-code__lang">${language ? escapeHtml(language) : 'text'}</span>` +
      `<button type="button" class="ark-md-code__copy" data-md-copy data-copy-source="${escapeAttr(text)}" aria-label="copy">` +
      `<span class="ark-md-code__copy-icon" aria-hidden="true"></span></button></div>` +
      `<pre><code class="hljs"${langClass}>${body}</code></pre></div>`
    )
  }
}

marked.use({ gfm: true, breaks: false, async: false, renderer })
// CJK emphasis fix: CommonMark leaves `**中文**` as literal asterisks when the
// flanking characters are CJK (commonmark/commonmark-spec#650). This extension
// applies the upstream CJK-friendly spec; Latin text is untouched.
marked.use(markedCjkFriendly())

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

/** Attribute-safe escaping: escape then strip line breaks entirely. */
function escapeAttr(text: string): string {
  return escapeHtml(text).replaceAll(/\r?\n/g, '&#10;')
}

/** Parse, highlight and sanitize in one step; always synchronous. */
export function renderMarkdown(text: string): string {
  // `async: false` keeps the return type a plain string: the renderer is a
  // synchronous projection step, never an async pipeline.
  const html = marked.parse(text, { async: false })
  // `data-*` attributes survive DOMPurify's defaults (ALLOW_DATA_ATTR), so the
  // copy button's raw source needs no extra allow-list here.
  return DOMPurify.sanitize(html)
}
