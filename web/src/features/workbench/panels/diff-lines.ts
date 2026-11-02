/**
 * Minimal unified-diff parser for the workspace `patch` field.
 *
 * The backend caps patch bytes and flags binary files explicitly, so this only
 * ever sees ordinary `git diff` text. Anything unparseable degrades to a
 * "meta" line instead of being dropped.
 */
export type DiffLineType = 'context' | 'add' | 'remove' | 'meta'

export type DiffLine = {
  type: DiffLineType
  text: string
  oldNumber: number | null
  newNumber: number | null
}

/** File-level header lines (`---`, `+++`, `diff --git`, `index`). */
function isFileHeader(line: string): boolean {
  return (
    line.startsWith('diff --git ') ||
    line.startsWith('index ') ||
    line.startsWith('--- ') ||
    line.startsWith('+++ ') ||
    line.startsWith('old mode ') ||
    line.startsWith('new mode ') ||
    line.startsWith('similarity index ') ||
    line.startsWith('rename from ') ||
    line.startsWith('rename to ')
  )
}

export function parseUnifiedDiff(patch: string): DiffLine[] {
  const lines: DiffLine[] = []
  let oldNumber: number | null = null
  let newNumber: number | null = null

  // git patches end with a newline; the final '' from split is an artifact, not
  // an empty context row (real empty context lines arrive as a single space).
  const rawLines = patch.endsWith('\n') ? patch.slice(0, -1).split('\n') : patch.split('\n')

  for (const raw of rawLines) {
    if (raw === '') continue
    if (raw.startsWith('@@')) {
      const match = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(raw)
      if (match) {
        oldNumber = Number(match[1])
        newNumber = Number(match[2])
        lines.push({ type: 'meta', text: raw, oldNumber: null, newNumber: null })
        continue
      }
      lines.push({ type: 'meta', text: raw, oldNumber: null, newNumber: null })
      continue
    }
    if (isFileHeader(raw)) {
      lines.push({ type: 'meta', text: raw, oldNumber: null, newNumber: null })
      continue
    }
    if (raw.startsWith('+')) {
      lines.push({ type: 'add', text: raw.slice(1), oldNumber: null, newNumber: newNumber })
      if (newNumber != null) newNumber += 1
      continue
    }
    if (raw.startsWith('-')) {
      lines.push({ type: 'remove', text: raw.slice(1), oldNumber: oldNumber, newNumber: null })
      if (oldNumber != null) oldNumber += 1
      continue
    }
    if (raw.startsWith('\\')) {
      // "\ No newline at end of file" — presentation-only.
      lines.push({ type: 'meta', text: raw, oldNumber: null, newNumber: null })
      continue
    }
    lines.push({ type: 'context', text: raw.startsWith(' ') ? raw.slice(1) : raw, oldNumber: oldNumber, newNumber: newNumber })
    if (oldNumber != null) oldNumber += 1
    if (newNumber != null) newNumber += 1
  }
  return lines
}

/** Added/removed counts for the summary chip; O(lines), no allocation. */
export function diffLineCounts(lines: readonly DiffLine[]): { additions: number; deletions: number } {
  let additions = 0
  let deletions = 0
  for (const line of lines) {
    if (line.type === 'add') additions += 1
    else if (line.type === 'remove') deletions += 1
  }
  return { additions, deletions }
}
