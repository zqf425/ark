/**
 * Skill mention palette, floating above the composer toolbar like the mainstream
 * agent canvases' slash menu: one row per skill (mono `/id`, display name,
 * one-line description) and a footer with the rescan action and the load-warning
 * count. Keyboard: ArrowUp/ArrowDown move the highlight, Enter inserts the
 * highlighted skill, Escape closes (owned by the parent).
 *
 * Two modes:
 * - `menu`: opened from the `+` menu. Carries its own search field.
 * - `token`: summoned by typing `/` in the draft. The typed text *is* the
 *   filter, so the search field is hidden; the parent owns query and highlight.
 *
 * Kept as a leaf component so the Composer only owns open/close state and the
 * insertion; the palette owns nothing durable, which resets on remount (the
 * palette only exists while open).
 */
import { useMemo, useState } from 'react'
import { useI18n } from '../../../shared/i18n'
import type { MessageKey } from '../../../shared/i18n/messages'
import type { SkillSummary } from '../../../shared/contracts'
import { filterSkills } from './skill-mention'

/** Row glyphs: a small bolt so a skill reads as "capability", not "file". */
function SkillGlyph(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true" focusable="false">
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

/** `source` is the wire enum; show the origin as a quiet chip like settings. */
function sourceLabelKey(source: SkillSummary['source']): MessageKey {
  return source === 'base' ? 'canvas.composer.skillsSourceBase' : 'canvas.composer.skillsSourceExtra'
}

function SkillRow({
  skill,
  active,
  onPick,
  onHover
}: {
  skill: SkillSummary
  active: boolean
  onPick: (id: string) => void
  onHover: () => void
}): JSX.Element {
  const { t } = useI18n()
  return (
    <button
      type="button"
      className={`ark-skill-palette__row${active ? ' is-active' : ''}`}
      role="option"
      aria-selected={active}
      title={skill.description ?? skill.name}
      onClick={() => onPick(skill.id)}
      onMouseMove={onHover}
    >
      <span className="ark-skill-palette__glyph" aria-hidden="true">
        <SkillGlyph />
      </span>
      <span className="ark-skill-palette__text">
        <span className="ark-skill-palette__top">
          <span className="ark-skill-palette__id">/{skill.id}</span>
          <span className="ark-skill-palette__name">{skill.name}</span>
        </span>
        {skill.description ? <span className="ark-skill-palette__desc">{skill.description}</span> : null}
      </span>
      <span className="ark-skill-palette__meta">
        {skill.legacy ? <span className="ark-skill-palette__tag">{t('canvas.composer.skillsLegacy')}</span> : null}
        {skill.source === 'thread_extra' ? (
          <span className="ark-skill-palette__tag">{t(sourceLabelKey(skill.source))}</span>
        ) : null}
        <span className="ark-skill-palette__insert" aria-hidden="true">
          {t('canvas.composer.skillsInsert')}
        </span>
      </span>
    </button>
  )
}

export type SkillPaletteMode = 'menu' | 'token'

/**
 * The skill list panel itself. `skills` is already the thread's loaded set
 * (base + extra); `null` means still loading, `[]` means nothing loaded.
 *
 * `token` mode is fully controlled: the parent passes `query` (the text after
 * the slash), `activeIndex` and the keyboard handlers, because the draft
 * textarea stays the input. `menu` mode is self-contained.
 */
export function SkillPalette({
  skills,
  warnings,
  canRefresh,
  mode,
  query = '',
  activeIndex = 0,
  onActiveIndexChange,
  onPick,
  onRefresh
}: {
  skills: SkillSummary[] | null
  warnings: number
  canRefresh: boolean
  mode: SkillPaletteMode
  /** token mode: the text typed after the slash (already lowercased or not). */
  query?: string
  /** token mode: the parent-owned highlight index. */
  activeIndex?: number
  onActiveIndexChange?: (index: number) => void
  onPick: (id: string) => void
  onRefresh: () => void
}): JSX.Element {
  const { t } = useI18n()
  const [menuQuery, setMenuQuery] = useState('')
  const [menuActive, setMenuActive] = useState(0)
  const isToken = mode === 'token'
  const effectiveQuery = isToken ? query : menuQuery
  const setActive = isToken ? (onActiveIndexChange ?? (() => {})) : setMenuActive
  const active = isToken ? activeIndex : menuActive

  const filtered = useMemo(() => (skills === null ? null : filterSkills(skills, effectiveQuery)), [skills, effectiveQuery])

  const list = filtered ?? []
  const clamped = list.length === 0 ? -1 : Math.min(active, list.length - 1)

  /** Arrow/Enter handling lives on the input so typing never loses focus. */
  const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>): void => {
    if (event.key === 'ArrowDown') {
      event.preventDefault()
      if (list.length > 0) setActive((clamped + 1) % list.length)
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      if (list.length > 0) setActive((clamped - 1 + list.length) % list.length)
    } else if (event.key === 'Enter') {
      event.preventDefault()
      const skill = list[clamped]
      if (skill) onPick(skill.id)
    }
  }

  return (
    <div className="ark-skill-palette" role="listbox" aria-label={t('canvas.composer.skillsAria')}>
      {!isToken ? (
        <div className="ark-skill-palette__search">
          <svg viewBox="0 0 24 24" width="13" height="13" aria-hidden="true" focusable="false">
            <circle cx="10.5" cy="10.5" r="6" fill="none" stroke="currentColor" strokeWidth="1.8" />
            <path d="m15.2 15.2 4.8 4.8" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
          </svg>
          <input
            className="ark-skill-palette__filter"
            type="text"
            value={menuQuery}
            placeholder={t('canvas.composer.skillsFilter')}
            aria-label={t('canvas.composer.skillsFilter')}
            // Autofocus so typing filters immediately; the parent keeps the
            // textarea's draft untouched while the palette is open.
            autoFocus
            onChange={(event) => {
              setMenuQuery(event.currentTarget.value)
              setMenuActive(0)
            }}
            onKeyDown={onKeyDown}
          />
        </div>
      ) : null}
      <div className="ark-skill-palette__list">
        {skills === null ? (
          <span className="ark-skill-palette__empty">{t('canvas.composer.skillsLoading')}</span>
        ) : list.length === 0 ? (
          <span className="ark-skill-palette__empty">
            {skills.length === 0 ? t('canvas.composer.skillsEmpty') : t('canvas.composer.skillsNoMatch')}
          </span>
        ) : (
          list.map((skill, index) => (
            <SkillRow
              key={`${skill.root}::${skill.id}`}
              skill={skill}
              active={index === clamped}
              onPick={onPick}
              onHover={() => setActive(index)}
            />
          ))
        )}
      </div>
      <div className="ark-skill-palette__foot">
        {warnings > 0 ? (
          <span className="ark-skill-palette__warn">{t('canvas.composer.skillWarnings', { count: warnings })}</span>
        ) : (
          <span className="ark-skill-palette__hint">{t('canvas.composer.skillsHint')}</span>
        )}
        {canRefresh ? (
          <button type="button" className="ark-linkbtn" onClick={onRefresh}>
            {t('canvas.composer.skillsRefresh')}
          </button>
        ) : null}
      </div>
    </div>
  )
}
