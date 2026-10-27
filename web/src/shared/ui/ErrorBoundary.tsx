/**
 * The one error boundary. A panel crash must not take down the shell
 * (design §7.5.3: "错误边界与降级").
 */
import { Component, type ErrorInfo, type ReactNode } from 'react'

type Props = {
  children: ReactNode
  /** Shown instead of the default card; receives the caught error. */
  fallback?: (error: Error, reset: () => void) => ReactNode
  label?: string
}

type State = { error: Error | null }

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null }

  static getDerivedStateFromError(error: Error): State {
    return { error }
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    // Keep the trace in the console: there is no remote crash reporter here.
    console.error('[ark-ui] 渲染错误', error, info.componentStack)
  }

  private reset = (): void => this.setState({ error: null })

  render(): ReactNode {
    const { error } = this.state
    if (!error) return this.props.children
    if (this.props.fallback) return this.props.fallback(error, this.reset)
    return (
      <div className="ark-error-boundary" role="alert">
        <p className="ark-error-boundary__title">{this.props.label ?? '该区域渲染失败'}</p>
        <p className="ark-error-boundary__message">{error.message}</p>
        <button className="ark-btn ark-btn--default ark-btn--sm" type="button" onClick={this.reset}>
          重试
        </button>
      </div>
    )
  }
}
