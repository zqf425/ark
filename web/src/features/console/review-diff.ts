/**
 * Pure helpers for the approved full-screen changes review (proposal A).
 *
 * `parseUnifiedDiff` already owns hunk numbering in features/workbench; this
 * module only pairs remove/add runs into aligned side-by-side rows and
 * tokenizes one source line for lightweight syntax colouring. No DOM, no
 * runtime routes — both are easy to unit test.
 */
import type { DiffLine } from '../workbench/panels/diff-lines'

export type SplitRow =
  | { kind: 'meta'; text: string }
  | { kind: 'pair'; left: DiffLine | null; right: DiffLine | null }

/**
 * Pair consecutive remove/add runs so both columns stay on one shared row
 * index. Meta lines (hunk headers) become full-width rows; context lines are
 * mirrored to both sides.
 */
export function buildSplitRows(lines: readonly DiffLine[]): SplitRow[] {
  const rows: SplitRow[] = []
  let removes: DiffLine[] = []
  let adds: DiffLine[] = []

  const flush = (): void => {
    const count = Math.max(removes.length, adds.length)
    for (let i = 0; i < count; i += 1) {
      rows.push({ kind: 'pair', left: removes[i] ?? null, right: adds[i] ?? null })
    }
    removes = []
    adds = []
  }

  for (const line of lines) {
    if (line.type === 'meta') {
      flush()
      rows.push({ kind: 'meta', text: line.text })
    } else if (line.type === 'remove') {
      removes.push(line)
    } else if (line.type === 'add') {
      adds.push(line)
    } else {
      flush()
      rows.push({ kind: 'pair', left: line, right: line })
    }
  }
  flush()
  return rows
}

export type Token = { text: string; cls: string | null }

const KEYWORDS = new Set([
  'let', 'mut', 'while', 'if', 'else', 'match', 'return', 'pub', 'async', 'fn',
  'loop', 'in', 'use', 'const', 'struct', 'enum', 'impl', 'self', 'true', 'false',
  'as', 'where', 'for', 'await', 'move', 'ref', 'break', 'continue', 'dyn', 'mod',
  'trait', 'type', 'static', 'unsafe', 'extern', 'crate', 'super'
])

const TOKEN_RE = /(\/\/.*)|("(?:[^"\\]|\\.)*")|(\b\d+\b)|([A-Za-z_][A-Za-z0-9_]*!?)/g

/** Small, dependency-free tokenizer whose span classes map to --tk-* tokens. */
export function tokenize(text: string): Token[] {
  const tokens: Token[] = []
  let last = 0
  let match: RegExpExecArray | null
  TOKEN_RE.lastIndex = 0
  while ((match = TOKEN_RE.exec(text)) !== null) {
    const value = match[0] ?? ''
    if (match.index > last) tokens.push({ text: text.slice(last, match.index), cls: null })
    let cls: string | null = null
    if (match[1]) cls = 'tk-cm'
    else if (match[2]) cls = 'tk-str'
    else if (match[3]) cls = 'tk-num'
    else if (value.endsWith('!')) cls = 'tk-mac'
    else if (KEYWORDS.has(value)) cls = 'tk-kw'
    else if (/^[A-Z]/.test(value)) cls = 'tk-ty'
    else if (text[TOKEN_RE.lastIndex] === '(') cls = 'tk-fn'
    tokens.push({ text: value, cls })
    last = TOKEN_RE.lastIndex
  }
  if (last < text.length) tokens.push({ text: text.slice(last), cls: null })
  return tokens
}
