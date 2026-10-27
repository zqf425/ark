import { useCodeSessionStore } from '../state/store'
import { useRuntimeStatus } from '../hooks/use-runtime-status'
import { ViewSwitch } from '../../workbench/components/ViewSwitch'

const ONLINE_LABEL = {
  unknown: '检测中',
  online: '在线',
  offline: '离线'
} as const

/**
 * Top status bar. Liveness and version come from the runtime
 * (`GET /health` + `GET /v1/runtime/info`), never from a build constant.
 */
export function RuntimeStatusBar(): JSX.Element {
  const online = useCodeSessionStore((s) => s.online)
  const healthVersion = useCodeSessionStore((s) => s.healthVersion)
  const runtimeVersion = useCodeSessionStore((s) => s.runtimeVersion)
  const dataDir = useCodeSessionStore((s) => s.dataDir)
  const checkedAt = useCodeSessionStore((s) => s.checkedAt)
  const healthError = useCodeSessionStore((s) => s.healthError)
  const streamStatus = useCodeSessionStore((s) => s.streamStatus)
  const { refresh } = useRuntimeStatus()

  const version = runtimeVersion ?? healthVersion ?? 'unknown'

  return (
    <header className="ark-statusbar">
      <div className="ark-statusbar__brand">
        <span className="ark-statusbar__logo">ark</span>
        <span className="ark-statusbar__mode">Code</span>
      </div>

      <ViewSwitch />

      <div className="ark-statusbar__group">
        <span className={`ark-dot ark-dot--${online === 'online' ? 'ok' : online === 'offline' ? 'bad' : 'idle'}`} />
        <span className="ark-statusbar__label">{ONLINE_LABEL[online]}</span>
        <span className="ark-statusbar__sep">·</span>
        <span className="ark-statusbar__label">v{version}</span>
        <span className="ark-statusbar__sep">·</span>
        <span className="ark-statusbar__label">stream {streamStatus}</span>
        {dataDir ? (
          <>
            <span className="ark-statusbar__sep">·</span>
            <code className="ark-statusbar__path" title={dataDir}>
              {dataDir}
            </code>
          </>
        ) : null}
      </div>

      <div className="ark-statusbar__group ark-statusbar__group--end">
        {healthError ? <span className="ark-statusbar__error">{healthError}</span> : null}
        {checkedAt ? (
          <span className="ark-statusbar__label">
            上次检测 {new Date(checkedAt).toLocaleTimeString('zh-CN', { hour12: false })}
          </span>
        ) : null}
        <button type="button" className="ark-linkbtn" onClick={() => void refresh()}>
          重新检测
        </button>
      </div>
    </header>
  )
}
