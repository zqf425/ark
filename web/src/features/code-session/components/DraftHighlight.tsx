/**
 * Draft mention highlight: a mirror layer under the composer's textarea.
 *
 * A textarea cannot style its own text, so the mainstream approach applies:
 * the same draft is painted twice — this layer behind, the real textarea on
 * top with its text opaque. The mirror recognizes complete `/id` mentions
 * (against the thread's loaded skills) and the in-progress token the caret is
 * inside, rendering both as chips. The textarea keeps its transparent text
 * color only over chip spans? No — it keeps its normal opaque color; the chip
 * behind shows through as a soft wash around identical glyphs, which is why
 * both layers MUST share the exact same font metrics and padding (one shared
 * class, `.ark-composer__editor-font`).
 *
 * Rendering `null` when the draft has nothing recognizable keeps an ordinary
 * draft at zero cost — no extra DOM, no layout shift.
 */
import { memo } from 'react'
import type { SkillSummary } from '../../../shared/contracts'
import { splitSkillMentions, type MentionToken } from './skill-mention'

function MentionGlyph(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="11" height="11" aria-hidden="true" focusable="false">
      <path
        d="M13 2.5 4.5 13.5H11L9.8 21.5 19.5 9.5H13l1.4-7Z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.7"
        strokeLinejoin="round"
      />
    </svg>
  )
}

export const DraftHighlight = memo(function DraftHighlight({
  text,
  skills,
  token
}: {
  text: string
  skills: SkillSummary[] | null
  /** The in-progress token under the caret, if the slash palette is open. */
  token: MentionToken | null
}): JSX.Element | null {
  if (!text) return null
  const segments = skills && skills.length > 0 ? splitSkillMentions(text, skills) : [{ kind: 'text' as const, value: text }]

  return (
    <div className="ark-composer__editor-font ark-composer__highlight" aria-hidden="true">
      {segments.map((segment, index) => {
        if (segment.kind === 'mention') {
          return (
            <span key={`m-${index}-${segment.value}`} className="ark-mention">
              <span className="ark-mention__glyph">
                <MentionGlyph />
              </span>
              {segment.value}
            </span>
          )
        }
        // Plain text may still contain the open token; chip it when present.
        if (token && segment.value.length > token.start) {
          const parts: JSX.Element[] = []
          let cursor = 0
          let offset = 0
          // Walk the segment, splitting at token bounds relative to the draft.
          while (cursor < segment.value.length) {
            const segStartGlobal = offset
            const segEndGlobal = offset + (segment.value.length - cursor)
            if (segEndGlobal > token.start && segStartGlobal < token.end) {
              const before = segment.value.slice(cursor, Math.max(0, token.start - segStartGlobal))
              if (before) parts.push(<span key={`t-${index}-${parts.length}`}>{before}</span>)
              const overlapStart = Math.max(cursor, token.start - segStartGlobal)
              const overlapEnd = Math.min(segment.value.length, token.end - segStartGlobal)
              const overlap = segment.value.slice(overlapStart, overlapEnd)
              if (overlap) {
                parts.push(
                  <span key={`k-${index}-${parts.length}`} className="ark-mention ark-mention--typing">
                    <span className="ark-mention__glyph">
                      <MentionGlyph />
                    </span>
                    {overlap}
                  </span>
                )
              }
              cursor = overlapEnd
            } else {
              const until = segEndGlobal <= token.start ? segment.value.length - cursor : token.start - segStartGlobal
              parts.push(<span key={`t-${index}-${parts.length}`}>{segment.value.slice(cursor, cursor + until)}</span>)
              cursor += until
            }
          }
          return <span key={`s-${index}`}>{parts}</span>
        }
        return <span key={`s-${index}`}>{segment.value}</span>
      })}
      {/* A trailing newline collapses in the mirror but not the textarea; keep
          the heights identical so the last line never misaligns. */}
      {'\u200b'}
    </div>
  )
})
