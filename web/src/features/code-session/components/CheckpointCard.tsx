import { memo } from 'react'
import type { ThreadItem } from '../../../shared/contracts'
import { useI18n } from '../../../shared/i18n'

/**
 * Durable compaction boundary, rendered the way mainstream coding agents do
 * it: one quiet collapsed line ("context compacted"), never the raw handoff
 * prose in the stream — a 2k-character model-facing note shown verbatim reads
 * like a runtime error to a user resuming a session.
 *
 * The full checkpoint text stays one click away inside the same card: it is
 * the authoritative record of what the next model context contains, so it is
 * presented verbatim (like reasoning), not paraphrased by the client.
 */
export const CheckpointCard = memo(function CheckpointCard({ item }: { item: ThreadItem }): JSX.Element {
  const { t } = useI18n()
  const text = item.text ?? ''
  // The tail note the reactor appends ("[438 earlier messages compacted]").
  const folded = /(?:^|\n)\[(\d+) earlier messages compacted\]/.exec(text)
  return (
    <details className="ark-checkpoint">
      <summary className="ark-checkpoint__summary">
        <span>{t('canvas.checkpoint.title')}</span>
        {folded ? (
          <span className="ark-checkpoint__meta">
            {t('canvas.checkpoint.folded', { count: Number(folded[1]) })}
          </span>
        ) : null}
      </summary>
      <pre className="ark-checkpoint__body">{text}</pre>
    </details>
  )
})
