/** Uniform empty state: icon-less, one title, one optional hint and action. */
import type { ReactNode } from 'react'

export type EmptyStateProps = {
  title: string
  hint?: string
  action?: ReactNode
}

export function EmptyState({ title, hint, action }: EmptyStateProps): JSX.Element {
  return (
    <div className="ark-empty">
      <p className="ark-empty__title">{title}</p>
      {hint ? <p className="ark-empty__hint">{hint}</p> : null}
      {action ? <div className="ark-empty__action">{action}</div> : null}
    </div>
  )
}
