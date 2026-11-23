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
 * The chip shares the editor's exact font metrics, so the wash painted behind
 * sits under the textarea's own glyphs without shifting them. It is a quiet
 * inline mark (violet rule + wash), not a box that competes with prose. The
 * component renders the verbatim token as the mark's only content.
 *
 * Rendering `null` when the draft has nothing recognizable keeps an ordinary
 * draft at zero cost — no extra DOM, no layout shift.
 */
import { memo } from 'react'
import type { SkillSummary } from '../../../shared/contracts'
import { splitSkillMentions, type MentionToken } from './skill-mention'

/**
 * One mirror mention mark. It renders the verbatim token inline with the same
 * font metrics as the textarea; the violet rule/wash comes from CSS, so there
 * is no glyph, spacer or metric compensation to keep in sync.
 */
function MirrorMention({ value, typing }: { value: string; typing: boolean }): JSX.Element {
  return <span className={typing ? 'ark-mention ark-mention--typing' : 'ark-mention'}>{value}</span>
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
  let segmentStart = 0

  return (
    <div className="ark-composer__editor-font ark-composer__highlight" aria-hidden="true">
      {segments.map((segment, index) => {
        const start = segmentStart
        const end = start + segment.value.length
        segmentStart = end
        if (segment.kind === 'mention') {
          return <MirrorMention key={`m-${index}-${segment.value}`} value={segment.value} typing={false} />
        }
        // The token uses draft-global offsets while this segment is local. A
        // direct three-way split guarantees progress and keeps text after a
        // token in the middle of the draft; the previous cursor loop could
        // remain forever at token.end and freeze the WebKit main thread.
        if (token && token.start < end && token.end > start) {
          const tokenStart = Math.max(0, token.start - start)
          const tokenEnd = Math.min(segment.value.length, token.end - start)
          const before = segment.value.slice(0, tokenStart)
          const overlap = segment.value.slice(tokenStart, tokenEnd)
          const after = segment.value.slice(tokenEnd)
          return (
            <span key={`s-${index}`}>
              {before}
              {overlap ? <MirrorMention value={overlap} typing /> : null}
              {after}
            </span>
          )
        }
        return <span key={`s-${index}`}>{segment.value}</span>
      })}
      {/* A trailing newline collapses in the mirror but not the textarea; keep
          the heights identical so the last line never misaligns. */}
      {'\u200b'}
    </div>
  )
})
