/**
 * Read-only file preview highlighting. Extension-based language pick with an
 * auto-detect fallback; output is highlight.js HTML, which escapes the source,
 * so it is safe to inject into a `<code>` element.
 */
import hljs from 'highlight.js/lib/common'
import { escapeHtml } from './markdown-render'

/** Same cap as markdown blocks: beyond this, plain escaped text. */
const MAX_HIGHLIGHT_CHARS = 100_000

const EXTENSION_LANGUAGES: Record<string, string> = {
  ts: 'typescript',
  tsx: 'typescript',
  js: 'javascript',
  jsx: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  rs: 'rust',
  py: 'python',
  rb: 'ruby',
  go: 'go',
  java: 'java',
  kt: 'kotlin',
  c: 'c',
  h: 'c',
  cpp: 'cpp',
  hpp: 'cpp',
  cs: 'csharp',
  swift: 'swift',
  json: 'json',
  yaml: 'yaml',
  yml: 'yaml',
  toml: 'ini',
  ini: 'ini',
  css: 'css',
  scss: 'scss',
  html: 'xml',
  xml: 'xml',
  svg: 'xml',
  md: 'markdown',
  sh: 'bash',
  bash: 'bash',
  zsh: 'bash',
  sql: 'sql',
  ps1: 'powershell'
}

function languageOf(path: string): string | undefined {
  const dot = path.lastIndexOf('.')
  if (dot < 0 || dot === path.length - 1) return undefined
  return EXTENSION_LANGUAGES[path.slice(dot + 1).toLowerCase()]
}

/** Highlighted HTML for a file's source, or escaped text when it is too big. */
export function highlightSource(source: string, path: string): string {
  if (source.length > MAX_HIGHLIGHT_CHARS) return escapeHtml(source)
  const language = languageOf(path)
  try {
    if (language && hljs.getLanguage(language)) {
      return hljs.highlight(source, { language, ignoreIllegals: true }).value
    }
    return hljs.highlightAuto(source).value
  } catch {
    return escapeHtml(source)
  }
}
