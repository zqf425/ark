/**
 * The one error boundary. A panel crash must not take down the shell
 * (design §7.5.3: "错误边界与降级").
 *
 * React error boundaries must be class components, so the localized copy is
 * injected by a thin functional wrapper: the class keeps its lifecycle, the
 * wrapper supplies the translated fallback title and retry label.
 */
import { Component, type ErrorInfo, type ReactNode } from 'react'
import { useI18n } from '../i18n'

type Props = {
  children: ReactNode
  /** Shown instead of the default card; receives the caught error. */
  fallback?: (error: Error, reset: () => void) => ReactNode
  /** Localized region name, e.g. "Thread list failed to render". */
  label?: string
  /** Localized copy used when `label` is absent. */
  defaultLabel: string
  retryLabel: string
}

type State = { error: Error | null }

class ErrorBoundaryImpl extends Component<Props, State> {
  state: State = { error: null }

  static getDerivedStateFromError(error: Error): State {
    return { error }
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    // Keep the trace in the console: there is no remote crash reporter here.
    console.error('[ark-ui] render error', error, info.componentStack)
  }

  private reset = (): void => this.setState({ error: null })

  render(): ReactNode {
    const { error } = this.state
    if (!error) return this.props.children
    if (this.props.fallback) return this.props.fallback(error, this.reset)
    return (
      <div className="ark-error-boundary" role="alert">
        <p className="ark-error-boundary__title">{this.props.label ?? this.props.defaultLabel}</p>
        <p className="ark-error-boundary__message">{error.message}</p>
        <button className="ark-btn ark-btn--default ark-btn--sm" type="button" onClick={this.reset}>
          {this.props.retryLabel}
        </button>
      </div>
    )
  }
}

export function ErrorBoundary({ children, fallback, label }: Omit<Props, 'defaultLabel' | 'retryLabel'>): JSX.Element {
  const { t } = useI18n()
  return (
    <ErrorBoundaryImpl
      fallback={fallback}
      label={label}
      defaultLabel={t('ui.renderFailed')}
      retryLabel={t('common.retry')}
    >
      {children}
    </ErrorBoundaryImpl>
  )
}
