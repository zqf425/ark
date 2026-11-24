/**
 * Background-child surfaces in the conversation column (Qoder-referenced
 * session layout): a banner above the canvas lists the thread's active
 * subagent runs — pulse + 后台 tag + label + live elapsed — and a hint above
 * the composer tells the user they can keep sending messages. Both read the
 * same cached per-thread list (`useChildActivity`); a row click opens the
 * inspector's subagents panel, where abort/steer/resume already live.
 */
import type { ChildRunSummary } from '../../../shared/contracts'
import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../state/store'
import { isActiveChildRun, useChildActivity } from '../hooks/use-child-activity'
import { useClockTick } from './use-clock-tick'
import { useDurationFormat } from '../hooks/use-duration-words'
import { childRunDuration } from './child-run-meta'

/** Active runs beyond this fold into one "more" entry; the panel has them all. */
const BANNER_VISIBLE = 3

function BannerRow({
  child,
  nowMs,
  onOpen
}: {
  child: ChildRunSummary
  nowMs: number
  onOpen: () => void
}): JSX.Element {
  const { t } = useI18n()
  const durationMs = childRunDuration(child, nowMs)
  const clock = useDurationFormat(durationMs ?? 0, durationMs != null)
  return (
    <button type="button" className="ark-childbanner__row" title={t('canvas.childBanner.open')} onClick={onOpen}>
      <span className="ark-dot ark-dot--live" aria-hidden="true" />
      <span className="ark-childbanner__tag">{t('canvas.childBanner.tag')}</span>
      <span className="ark-childbanner__label">{child.label}</span>
      <span className="ark-childbanner__state">
        {child.status === 'queued'
          ? t('canvas.childBanner.queued')
          : t('canvas.childBanner.running', { duration: clock.span })}
      </span>
    </button>
  )
}

export function ChildActivityBanner({ threadId }: { threadId: string }): JSX.Element | null {
  const { t } = useI18n()
  const runs = useChildActivity(threadId)
  const active = (runs ?? []).filter(isActiveChildRun)
  // One tick per second, only while something is actually active.
  const nowMs = useClockTick(active.length > 0)
  const openPanel = useCodeSessionStore((s) => s.openPanel)
  if (active.length === 0) return null
  const open = (): void => openPanel('subagents')
  return (
    <div className="ark-childbanner" role="status">
      {active.slice(0, BANNER_VISIBLE).map((child) => (
        <BannerRow key={child.id} child={child} nowMs={nowMs} onOpen={open} />
      ))}
      {active.length > BANNER_VISIBLE ? (
        <button type="button" className="ark-childbanner__more" onClick={open}>
          {t('canvas.childBanner.more', { count: active.length - BANNER_VISIBLE })}
        </button>
      ) : null}
    </div>
  )
}

/** Composer-side hint: the main conversation stays usable while children run. */
export function ChildActivityHint({ threadId }: { threadId: string }): JSX.Element | null {
  const { t } = useI18n()
  const runs = useChildActivity(threadId)
  const active = (runs ?? []).filter(isActiveChildRun).length
  if (active === 0) return null
  return (
    <p className="ark-childhint" role="status">
      {t('canvas.composer.childHint', { count: active })}
    </p>
  )
}