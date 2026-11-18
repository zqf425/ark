/**
 * Jump-rail turn label preview (canvas right edge).
 *
 * The rail lists one entry per turn; its tooltip used to say only "turn N",
 * which forces a hover-and-guess when many turns look alike. The user's own
 * message is the most recognizable anchor, so the label appends its head.
 *
 * Kept free of React so the truncation rules are unit-testable:
 * - input is trimmed and inner line breaks collapse (a multi-line paste must
 *   not produce an empty-looking tooltip);
 * - the limit counts user-perceived characters (`Array.from`), so a surrogate
 *   pair or a composed emoji is never cut in half;
 * - `...` marks truncation and is ASCII per the repo's default-ASCII rule.
 */

/** Max user-perceived characters of the user input shown in the rail label. */
export const JUMPRAIL_PREVIEW_LIMIT = 15

/** Collapse line breaks so multi-line input still yields one readable line. */
function normalize(text: string): string {
  return text.trim().replace(/\s*\n+\s*/g, ' ')
}

export function turnLabelPreview(text: string | null | undefined): string {
  const normalized = text ? normalize(text) : ''
  if (!normalized) return ''
  const graphemes = Array.from(normalized)
  if (graphemes.length <= JUMPRAIL_PREVIEW_LIMIT) return normalized
  return `${graphemes.slice(0, JUMPRAIL_PREVIEW_LIMIT).join('')}...`
}
