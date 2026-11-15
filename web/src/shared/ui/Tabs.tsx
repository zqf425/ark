/** Minimal tab strip used by the workbench panel slot. */
import type { ReactNode } from 'react'

export type TabItem<T extends string> = {
  id: T
  label: string
  /** Rendered verbatim after the label; the caller owns badge styling. */
  badge?: ReactNode
}

export type TabsProps<T extends string> = {
  items: TabItem<T>[]
  active: T
  onChange: (id: T) => void
  label: string
}

export function Tabs<T extends string>({ items, active, onChange, label }: TabsProps<T>): JSX.Element {
  return (
    <div className="ark-tabs" role="tablist" aria-label={label}>
      {items.map((item) => (
        <button
          key={item.id}
          type="button"
          role="tab"
          aria-selected={item.id === active}
          className={`ark-tabs__tab${item.id === active ? ' is-active' : ''}`}
          onClick={() => onChange(item.id)}
        >
          {item.label}
          {item.badge}
        </button>
      ))}
    </div>
  )
}
