import { useCodeSessionStore } from '../state/store'
import { useRuntimeStatus } from '../hooks/use-runtime-status'
import { ViewSwitch } from '../../workbench/components/ViewSwitch'

const ONLINE_LABEL = {
  unknown: '检测中',
  online: '在线',
  offline: '离线'
} as const

const STREAM_LABEL: Record<string, string> = {
  connecting: '连接中',
  open: '已同步',
  reconnecting: '重连中',
  closed: '事件流已断开'
}

/**
 * Top status bar. Liveness and version come from the runtime
 * (`GET /health` + `GET /v1/runtime/info`), never from a build constant.
 */
export function RuntimeStatusBar(): JSX.Element {
  const online = useCodeSessionStore((s) => s.online)
  const healthError = useCodeSessionStore((s) => s.healthError)
  const streamStatus = useCodeSessionStore((s) => s.streamStatus)
  const { refresh } = useRuntimeStatus()
  const refreshLabel = healthError ? `重新检查运行时。上次失败：${healthError}` : '重新检查运行时'

  return (
    <header className="ark-statusbar">
      <div className="ark-statusbar__brand">
        <span className="ark-statusbar__logo">ark</span>
      </div>

      <ViewSwitch />

      <div
        className="ark-statusbar__group"
        role="status"
        aria-live="polite"
        aria-label={[ONLINE_LABEL[online], STREAM_LABEL[streamStatus] ?? streamStatus, healthError].filter(Boolean).join('，')}
      >
        <span className={`ark-dot ark-dot--${online === 'online' ? 'ok' : online === 'offline' ? 'bad' : 'idle'}`} aria-hidden="true" />
        <span className="ark-statusbar__label">{ONLINE_LABEL[online]}</span>
        <span className={`ark-statusbar__label${streamStatus === 'open' ? '' : ' is-warning'}`}>
          {STREAM_LABEL[streamStatus] ?? streamStatus}
        </span>
        {healthError ? <span className="ark-statusbar__error">{healthError}</span> : null}
      </div>

      <div className="ark-statusbar__group ark-statusbar__group--end">
        <button
          type="button"
          className="ark-statusbar__refresh"
          title={refreshLabel}
          aria-label={refreshLabel}
          onClick={() => void refresh()}
        >
          ↻
        </button>
      </div>
    </header>
  )
}
