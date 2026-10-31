/**
 * Small presentational scaffolding shared by the settings sections.
 *
 * These are layout/typography wrappers only: colour, radius and spacing all
 * come from the global tokens, and no interactive primitive is reimplemented
 * here (buttons/toasts stay in shared/ui).
 */
import type { ReactNode } from 'react'
import { useI18n } from '../../../shared/i18n'

export type SettingsSectionProps = {
  title: string
  desc?: string
  actions?: ReactNode
  children: ReactNode
}

/** One titled card. Section state is always owned by the caller. */
export function SettingsSection({ title, desc, actions, children }: SettingsSectionProps): JSX.Element {
  return (
    <section className="ark-settings__section">
      <div className="ark-settings__head">
        <h2 className="ark-settings__title">{title}</h2>
        {desc ? <p className="ark-settings__desc">{desc}</p> : null}
        {actions ? <div className="ark-card__actions">{actions}</div> : null}
      </div>
      {children}
    </section>
  )
}

export type NoticeProps = {
  tone?: 'warn' | 'error'
  title?: string
  items?: string[]
  children?: ReactNode
}

/** Inline, non-transient message. Failures must stay on screen, not only toast. */
export function Notice({ tone = 'warn', title, items, children }: NoticeProps): JSX.Element {
  return (
    <div className={`ark-settings__notice${tone === 'error' ? ' ark-settings__notice--error' : ''}`} role="alert">
      {title ? <span className="ark-settings__notice-title">{title}</span> : null}
      {children}
      {items && items.length > 0 ? (
        <ul className="ark-settings__notice-list">
          {items.map((item) => (
            <li key={item}>{item}</li>
          ))}
        </ul>
      ) : null}
    </div>
  )
}

/** Loading placeholder so a section never flashes empty while fetching. */
export function Skeleton({ rows = 3 }: { rows?: number }): JSX.Element {
  return (
    <div className="ark-skeleton" aria-hidden="true">
      {Array.from({ length: rows }, (_, index) => (
        <div
          key={index}
          className={`ark-skeleton__bar${index % 3 === 1 ? ' ark-skeleton__bar--wide' : index % 3 === 2 ? ' ark-skeleton__bar--narrow' : ''}`}
        />
      ))}
    </div>
  )
}

export type KeyValueRow = { key: string; value: ReactNode }

/** Read-only facts (port, data_dir, version, ...). */
export function KeyValue({ rows }: { rows: KeyValueRow[] }): JSX.Element {
  return (
    <dl className="ark-kv">
      {rows.map((row) => (
        <div key={row.key} style={{ display: 'contents' }}>
          <dt className="ark-kv__k">{row.key}</dt>
          <dd className="ark-kv__v">{row.value}</dd>
        </div>
      ))}
    </dl>
  )
}

/** Non-blocking load failure with a retry affordance. */
export function LoadError({ message, onRetry }: { message: string; onRetry: () => void }): JSX.Element {
  const { t } = useI18n()
  return (
    <div className="ark-settings__row">
      <span className="ark-settings__hint ark-settings__hint--error">{message}</span>
      <button type="button" className="ark-linkbtn" onClick={onRetry}>
        {t('common.retry')}
      </button>
    </div>
  )
}
