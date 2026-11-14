/**
 * Renderer-side recognition of skill mentions in user message text.
 *
 * The durable item text is verbatim user input; the runtime owns activation
 * (`ark-skills::activation`). This projection only decides what to *render*:
 * `/id` tokens that look like a mention become chips so the mention reads as a
 * capability invocation, matching how the composer's palette inserts them.
 *
 * Deliberately conservative: a token is a mention only when a known skill id
 * matches exactly after the slash. Unknown `/words` (paths, commands, URLs)
 * render as plain text — the previous behavior.
 */
import type { SkillSummary } from '../../../shared/contracts'

export type MentionSegment =
  | { kind: 'text'; value: string }
  | { kind: 'mention'; value: string; skill: SkillSummary }

/** The same charset as a lookahead, so boundary checks share one definition. */
const TOKEN_CHAR = /[\w.-]/

/**
 * Split `text` into plain segments and skill mentions, preserving order.
 *
 * `skills` is the thread's loaded set (base + extra). Matching is
 * case-insensitive (the activation path lowercases too) and requires a word
 * boundary after the id, so `/kun-new-session` does not light up inside
 * `/kun-new-sessions-2`. Overlapping ids resolve to the longest match.
 */
export function splitSkillMentions(text: string, skills: readonly SkillSummary[]): MentionSegment[] {
  if (!text) return []
  const byLower = new Map<string, SkillSummary>()
  for (const skill of skills) byLower.set(skill.id.toLowerCase(), skill)
  // Longest ids first so `/kun-new-session` wins over a shorter shared prefix.
  const ids = [...byLower.keys()].sort((a, b) => b.length - a.length)
  if (ids.length === 0) return [{ kind: 'text', value: text }]

  const segments: MentionSegment[] = []
  let cursor = 0
  while (cursor < text.length) {
    const slash = text.indexOf('/', cursor)
    // No further slash, or it opens the last token with nothing after it.
    if (slash === -1) break
    const rest = text.slice(slash + 1)
    const token = /^[\w.-]+/.exec(rest)?.[0]
    if (!token) {
      // A bare slash (URLs, `a / b`): emit up to and including it as text.
      push(text.slice(cursor, slash + 1))
      cursor = slash + 1
      continue
    }
    const lower = token.toLowerCase()
    const hit = ids.find((id) => lower === id || lower.startsWith(`${id}.`))
    const boundary = (() => {
      const next = rest[token.length]
      return next === undefined || !/[\w.-]/.test(next)
    })()
    const skill = hit && boundary ? byLower.get(hit) : undefined
    if (!skill) {
      push(text.slice(cursor, slash + token.length + 1))
      cursor = slash + token.length + 1
      continue
    }
    push(text.slice(cursor, slash))
    segments.push({ kind: 'mention', value: `/${token}`, skill })
    cursor = slash + token.length + 1
  }
  if (cursor < text.length) push(text.slice(cursor))
  return segments

  /** Append a non-empty run of plain text, merging with the previous one. */
  function push(value: string): void {
    if (!value) return
    const last = segments[segments.length - 1]
    if (last && last.kind === 'text') last.value += value
    else segments.push({ kind: 'text', value })
  }
}

/** The in-progress mention under the caret, or `null` when none is open. */
export type MentionToken = {
  /** Where the `/` sits in the draft. */
  start: number
  /** End of the typed token (caret position when the token is under the caret). */
  end: number
  /** The typed query after the slash, lowercased. Empty for a bare `/`. */
  query: string
}

/**
 * The mention token the caret is inside, e.g. `"/kun-new"` with the caret
 * anywhere inside it.
 *
 * A token opens only at a word boundary: `/` must be at position 0 or follow
 * whitespace, so `foo/bar` (paths) and `https://x` (URLs) never summon the
 * palette. A token closes at the first whitespace — `a / b` is arithmetic, not
 * a mention.
 */
export function mentionTokenAt(text: string, caret: number): MentionToken | null {
  if (caret < 0 || caret > text.length) return null
  // The token must end at the caret: a caret parked after a space (`/pd `) is
  // outside it. The character left of the caret is the last typed char.
  const left = caret - 1
  if (left < 0) return null
  if (/\s/.test(text[left] ?? '')) return null
  // Scan back to the nearest whitespace; a `/` opening this run opens a token.
  let start = left
  while (start > 0 && !/\s/.test(text[start - 1] ?? '')) start -= 1
  if (text[start] !== '/') return null
  if (start > 0 && TOKEN_CHAR.test(text[start - 1] ?? '')) return null
  // No regex needed on the tail: reaching here means every char from the slash
  // to the caret is non-whitespace (the scan-back stopped at the first gap).
  return { start, end: caret, query: text.slice(start + 1, caret).toLowerCase() }
}

/** Case-insensitive substring filter over id, name and description. */
export function filterSkills(skills: readonly SkillSummary[], query: string): SkillSummary[] {
  const needle = query.trim().toLowerCase()
  if (!needle) return [...skills]
  return skills.filter((skill) =>
    [skill.id, skill.name, skill.description ?? ''].some((field) => field.toLowerCase().includes(needle))
  )
}
