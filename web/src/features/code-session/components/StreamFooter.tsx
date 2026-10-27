import { useCodeSessionStore } from '../state/store'
import { hasLiveItems } from '../state/selectors'

const STREAM_LABEL: Record<string, string> = {
  connecting: '连接中',
  open: '已连接',
  reconnecting: '重连中',
  closed: '未连接'
}

/**
 * Stream health strip under the message list. `seq` is the resume watermark:
 * showing it makes "why did I reconnect here" answerable from a screenshot.
 */
export function StreamFooter(): JSX.Element {
  const streamStatus = useCodeSessionStore((s) => s.streamStatus)
  const streamError = useCodeSessionStore((s) => s.streamError)
  const lastSeq = useCodeSessionStore((s) => s.lastSeq)
  const snapshot = useCodeSessionStore((s) => s)
  const live = hasLiveItems(snapshot)

  return (
    <div className="ark-streamfooter">
      <span className={`ark-dot ark-dot--${streamStatus === 'open' ? 'ok' : 'warn'}`} aria-hidden="true" />
      <span className="ark-streamfooter__label">{STREAM_LABEL[streamStatus] ?? streamStatus}</span>
      <span className="ark-streamfooter__meta">seq {lastSeq}</span>
      {live ? <span className="ark-streamfooter__meta is-live">生成中</span> : null}
      {streamError ? <span className="ark-streamfooter__error">{streamError}</span> : null}
    </div>
  )
}
