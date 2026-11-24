import { useEffect, useState } from 'react'
import type { ThreadItem } from '../../../shared/contracts'
import { useI18n, type MessageKey, type MessageParams } from '../../../shared/i18n'
import { useCodeSessionStore } from '../state/store'
import { formatElapsed, liveActivity, reasoningTrail } from '../state/live-activity'

/**
 * Live activity row at the attention point (the stream bottom).
 *
 * The work-process card keeps the durable history above the assistant rows,
 * but the canvas follows the newest row: during a long pure-reasoning stretch
 * nothing visible changes and a healthy stream reads as "hung". While the
 * turn runs, this row mirrors the *newest* streaming item — a reasoning trail
 * with an elapsed clock, the running tool, or plain generation — so the model's
 * direction is observable and steerable the moment it drifts.
 */
export function LiveActivity(): JSX.Element | null {
  const { t } = useI18n()
  const itemOrder = useCodeSessionStore((s) => s.itemOrder)
  const itemsById = useCodeSessionStore((s) => s.itemsById)
  const turnState = useCodeSessionStore((s) => s.turnState)
  const activeTurnId = useCodeSessionStore((s) => s.activeTurnId)

  const activity = liveActivity({ itemOrder, itemsById, turnState, activeTurnId })
  const live = activity !== null

  // One tick per second, only while something is actually streaming: the row
  // unmounts between rounds, so the timer never runs in a settled canvas.
  const [nowMs, setNowMs] = useState(() => Date.now())
  useEffect(() => {
    if (!live) return undefined
    setNowMs(Date.now())
    const timer = setInterval(() => setNowMs(Date.now()), 1000)
    return () => clearInterval(timer)
  }, [live])

  if (!activity) return null
  const { item } = activity
  return <LiveActivityRow item={item} nowMs={nowMs} t={t} />
}

/** Memoized row: per-second ticks must not re-render the whole turn above. */
function LiveActivityRow({
  item,
  nowMs,
  t
}: {
  item: ThreadItem
  nowMs: number
  t: (key: MessageKey, params?: MessageParams) => string
}): JSX.Element {
  const elapsed = formatElapsed(item.created_at, nowMs)

  if (item.kind === 'reasoning') {
    const trail = reasoningTrail(item.text ?? '')
    return (
      <div className="ark-liveactivity" role="status">
        <span className="ark-tool__status ark-tool__status--live" aria-hidden="true" />
        <span className="ark-liveactivity__label">{t('canvas.activity.thinking')}</span>
        {elapsed ? <span className="ark-liveactivity__meta">{elapsed}</span> : null}
        {trail ? <span className="ark-liveactivity__trail">{trail}</span> : null}
      </div>
    )
  }
  if (item.kind === 'tool_call') {
    return (
      <div className="ark-liveactivity" role="status">
        <span className="ark-tool__status ark-tool__status--live" aria-hidden="true" />
        <span className="ark-liveactivity__label">
          {t('canvas.activity.tool', { tool: item.tool?.tool ?? '' })}
        </span>
        {elapsed ? <span className="ark-liveactivity__meta">{elapsed}</span> : null}
      </div>
    )
  }
  return (
    <div className="ark-liveactivity" role="status">
      <span className="ark-tool__status ark-tool__status--live" aria-hidden="true" />
      <span className="ark-liveactivity__label">{t('canvas.activity.writing')}</span>
      {elapsed ? <span className="ark-liveactivity__meta">{elapsed}</span> : null}
    </div>
  )
}
