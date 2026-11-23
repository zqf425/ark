import { useI18n } from '../../../shared/i18n'
import type { ThreadItem } from '../../../shared/contracts'

/**
 * Collapsed history row for an answered user-input gate.
 *
 * A settled gate is conversation history, not a live control: the summary
 * reads like a normal chat turn ("已回答 · 提问预览"), so the exchanged
 * question-and-answer stays in the canvas at its turn without the answered
 * card from the live-gate channel re-pinning itself under the newest turn
 * forever. One collapsed line by default; expanding reveals the verbatim
 * prompt and the answer the model continued with.
 *
 * The row is a plain uncontrolled `<details>`: open/close is browser state,
 * and the phase reset (a row opened mid-run closes when the turn settles)
 * comes from the caller keying the row per phase, which remounts it — the
 * same remount-resets-local-state rule the Composer uses for thread switches.
 */
export function AnsweredGateRow({ item }: { item: ThreadItem }): JSX.Element {
  const { t } = useI18n()
  return (
    <details className="ark-answeredgate">
      <summary className="ark-answeredgate__head">
        <span className="ark-answeredgate__chevron" aria-hidden="true">
          ▸
        </span>
        <span className="ark-answeredgate__label">{t('canvas.userInput.answered')}</span>
        <span className="ark-answeredgate__preview">{item.text}</span>
      </summary>
      <div className="ark-answeredgate__body">
        <p className="ark-answeredgate__question">{item.text}</p>
        <p className="ark-answeredgate__answer">{item.detail}</p>
      </div>
    </details>
  )
}
