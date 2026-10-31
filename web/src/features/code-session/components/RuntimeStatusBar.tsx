import { useI18n, type MessageKey } from '../../../shared/i18n'
import { useCodeSessionStore } from '../state/store'
import { useRuntimeStatus } from '../hooks/use-runtime-status'
import { ViewSwitch } from '../../workbench/components/ViewSwitch'

const ONLINE_LABEL_KEYS: Record<'unknown' | 'online' | 'offline', MessageKey> = {
  unknown: 'status.online.unknown',
  online: 'status.online.online',
  offline: 'status.online.offline'
}

const STREAM_LABEL_KEYS: Record<string, MessageKey> = {
  connecting: 'status.stream.connecting',
  open: 'status.stream.open',
  reconnecting: 'status.stream.reconnecting',
  closed: 'status.stream.closed'
}

/**
 * Top status bar. Liveness and version come from the runtime
 * (`GET /health` + `GET /v1/runtime/info`), never from a build constant.
 * Every visible string is a catalog key so the bar follows the display language.
 */
export function RuntimeStatusBar(): JSX.Element {
  const { t } = useI18n()
  const online = useCodeSessionStore((s) => s.online)
  const healthError = useCodeSessionStore((s) => s.healthError)
  const streamStatus = useCodeSessionStore((s) => s.streamStatus)
  const { refresh } = useRuntimeStatus()

  const onlineLabel = t(ONLINE_LABEL_KEYS[online])
  const streamKey = STREAM_LABEL_KEYS[streamStatus]
  const streamLabel = streamKey ? t(streamKey) : streamStatus
  const refreshLabel = healthError ? t('status.refreshFailed', { error: healthError }) : t('status.refresh')

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
        aria-label={[onlineLabel, streamLabel, healthError].filter(Boolean).join(t('status.ariaSeparator'))}
      >
        <span className={`ark-dot ark-dot--${online === 'online' ? 'ok' : online === 'offline' ? 'bad' : 'idle'}`} aria-hidden="true" />
        <span className="ark-statusbar__label">{onlineLabel}</span>
        <span className={`ark-statusbar__label${streamStatus === 'open' ? '' : ' is-warning'}`}>
          {streamLabel}
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
