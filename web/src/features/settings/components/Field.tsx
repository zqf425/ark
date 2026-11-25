/**
 * Labelled form controls. One label style, one input style, one inline variant,
 * so every settings section looks the same without its own CSS.
 */
import { useState, type ChangeEvent, type ReactNode } from 'react'
import { useI18n } from '../../../shared/i18n'

export type FieldProps = {
  label: string
  hint?: ReactNode
  htmlFor?: string
  inline?: boolean
  /** Extra layout class for grid placement; the base styles stay shared. */
  className?: string
  children: ReactNode
}

export function Field({ label, hint, htmlFor, inline = false, className, children }: FieldProps): JSX.Element {
  return (
    <div className={`ark-field${inline ? ' ark-field--inline' : ''}${className ? ` ${className}` : ''}`}>
      <label className="ark-field__label" htmlFor={htmlFor}>
        {label}
      </label>
      {children}
      {hint ? <span className="ark-settings__hint">{hint}</span> : null}
    </div>
  )
}

export type TextFieldProps = {
  id: string
  label: string
  value: string
  onChange: (value: string) => void
  placeholder?: string
  hint?: ReactNode
  disabled?: boolean
  mono?: boolean
  type?: 'text' | 'password' | 'number'
  /** Masks the value by default and offers a show/hide toggle for the draft. */
  revealable?: boolean
}

/** Single-line text input bound to a string value. */
export function TextField({
  id,
  label,
  value,
  onChange,
  placeholder,
  hint,
  disabled = false,
  mono = false,
  type = 'text',
  revealable = false
}: TextFieldProps): JSX.Element {
  const { t } = useI18n()
  const [revealed, setRevealed] = useState(false)
  const handle = (event: ChangeEvent<HTMLInputElement>): void => onChange(event.target.value)
  const input = (
    <input
      id={id}
      className="ark-field__input"
      type={revealable ? (revealed ? 'text' : 'password') : type}
      value={value}
      placeholder={placeholder}
      disabled={disabled}
      spellCheck={false}
      autoComplete="off"
      style={mono ? { fontFamily: 'var(--ark-mono)', fontSize: 'calc(11.5px * var(--ark-scale))' } : undefined}
      onChange={handle}
    />
  )
  return (
    <Field label={label} hint={hint} htmlFor={id}>
      {revealable ? (
        <div className="ark-field__secret">
          {input}
          <button
            type="button"
            className="ark-linkbtn ark-field__secret-toggle"
            aria-pressed={revealed}
            disabled={disabled}
            onClick={() => setRevealed((current) => !current)}
          >
            {revealed ? t('common.hideSecret') : t('common.showSecret')}
          </button>
        </div>
      ) : input}
    </Field>
  )
}

export type SelectFieldProps<T extends string> = {
  id: string
  label: string
  value: T
  options: readonly T[]
  labels?: Partial<Record<T, string>>
  onChange: (value: T) => void
  hint?: ReactNode
  disabled?: boolean
}

/** Select over a literal union; the options array is the source of truth. */
export function SelectField<T extends string>({
  id,
  label,
  value,
  options,
  labels,
  onChange,
  hint,
  disabled = false
}: SelectFieldProps<T>): JSX.Element {
  return (
    <Field label={label} hint={hint} htmlFor={id}>
      <select
        id={id}
        className="ark-field__select"
        value={value}
        disabled={disabled}
        onChange={(event) => onChange(event.target.value as T)}
      >
        {options.map((option) => (
          <option key={option} value={option}>
            {labels?.[option] ?? option}
          </option>
        ))}
      </select>
    </Field>
  )
}

/** Checkbox with an inline label; used for `enabled` style switches. */
export function CheckField({
  label,
  checked,
  onChange,
  disabled = false
}: {
  label: string
  checked: boolean
  onChange: (checked: boolean) => void
  disabled?: boolean
}): JSX.Element {
  return (
    <label className="ark-check">
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(event) => onChange(event.target.checked)} />
      <span>{label}</span>
    </label>
  )
}
