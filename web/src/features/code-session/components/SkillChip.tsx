/**
 * Skill reference chip shared by the composer's selected-skill row and the
 * message bubbles: sparkles glyph + skill name, no fill, no border. The
 * verbatim mention (`/<id>`) stays the hover title so the wire text is visible.
 */
import { useI18n } from '../../../shared/i18n'

export function SparklesGlyph(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" focusable="false" className="ark-skillchip__icon">
      <path
        d="M10 3.5 11.6 8.4 16.5 10 11.6 11.6 10 16.5 8.4 11.6 3.5 10 8.4 8.4Z M18 14.5l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8Z M18 3l.6 1.4L20 5l-1.4.6L18 7l-.6-1.4L16 5l1.4-.6Z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.9"
        strokeLinejoin="round"
      />
    </svg>
  )
}

function CloseGlyph(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="12" height="12" aria-hidden="true" focusable="false">
      <path d="M6 6l12 12M18 6 6 18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </svg>
  )
}

export function SkillChip({
  id,
  name,
  onRemove
}: {
  id: string
  /** Display name; falls back to the id when the skill is not loaded. */
  name?: string
  /** Present only while the chip is editable. */
  onRemove?: () => void
}): JSX.Element {
  const { t } = useI18n()
  const label = name || id
  return (
    <span className="ark-skillchip" title={`/${id}`} data-skill-id={id}>
      <SparklesGlyph />
      <span className="ark-skillchip__name">{label}</span>
      {onRemove ? (
        <button
          type="button"
          className="ark-skillchip__remove"
          aria-label={t('canvas.composer.skillChipRemove', { name: label })}
          title={t('canvas.composer.skillChipRemove', { name: label })}
          onClick={onRemove}
        >
          <CloseGlyph />
        </button>
      ) : null}
    </span>
  )
}
