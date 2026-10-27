/**
 * Labelled form controls. One label style, one input style, one inline variant,
 * so every settings section looks the same without its own CSS.
 */
import type { ChangeEvent, ReactNode } from 'react'

export type FieldProps = {
  label: string
  hint?: ReactNode
  htmlFor?: string
  inline?: boolean
  children: ReactNode
}

export function Field({ label, hint, htmlFor, inline = false, children }: FieldProps): JSX.Element {
  return (
    <div className={`ark-field${inline ? ' ark-field--inline' : ''}`}>
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
  type = 'text'
}: TextFieldProps): JSX.Element {
  const handle = (event: ChangeEvent<HTMLInputElement>): void => onChange(event.target.value)
  return (
    <Field label={label} hint={hint} htmlFor={id}>
      <input
        id={id}
        className="ark-field__input"
        type={type}
        value={value}
        placeholder={placeholder}
        disabled={disabled}
        spellCheck={false}
        autoComplete="off"
        style={mono ? { fontFamily: 'var(--ark-mono)', fontSize: 11.5 } : undefined}
        onChange={handle}
      />
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
