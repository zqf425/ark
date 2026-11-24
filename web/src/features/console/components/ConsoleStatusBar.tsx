/**
 * Bottom status bar: liveness, workspace, change count, version and the ⌘K
 * hint. Every number comes from the runtime or the folded usage aggregate —
 * no build constants, no invented telemetry. Context fill has a single home in
 * the composer's context ring, so the bar deliberately does not repeat it.
 */
import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../../code-session/state/store'
import { useRuntimeStatusLabels } from '../../code-session/hooks/use-runtime-status-labels'
import { useViewStore } from '../../workbench/state/view-slice'
import type { ConsoleMeta } from '../hooks/use-console-meta'

export function ConsoleStatusBar({ meta }: { meta: ConsoleMeta }): JSX.Element {
  const { t } = useI18n()
  const { online, refreshLabel, refresh } = useRuntimeStatusLabels()
  const runtimeVersion = useCodeSessionStore((s) => s.runtimeVersion)
  const healthVersion = useCodeSessionStore((s) => s.healthVersion)
  const setCommandOpen = useViewStore((s) => s.setCommandOpen)

  const onlineLabelText = online === 'online'
    ? t('console.status.connected')
    : online === 'offline'
      ? t('console.status.offline')
      : t('console.status.checking')
  const dotClass = online === 'online'
    ? 'console-dot--live'
    : online === 'offline'
      ? 'console-dot--warn'
      : ''

  const version = runtimeVersion ?? healthVersion

  return (
    <footer className="console-status">
      <button
        type="button"
        className="console-status__button console-status__ok"
        onClick={() => void refresh()}
        title={refreshLabel}
        aria-label={refreshLabel}
      >
        <span className={`console-dot ${dotClass}`} aria-hidden="true" />
        {onlineLabelText}
      </button>

      <span className="console-status__item console-status__item--ellipsis" title={meta.workspacePath ?? ''}>
        <b>{meta.workspacePath ?? t('console.top.workspace')}</b>
      </span>

      <span className="console-status__item">
        {meta.changeCount > 0
          ? t('console.status.changes', { count: meta.changeCount })
          : t('console.status.changesClean')}
      </span>

      <span className="console-status__grow" />

      <span className="console-status__item console-status__version">{version ?? '—'}</span>

      <button
        type="button"
        className="console-status__button console-status__command"
        onClick={() => setCommandOpen(true)}
        title={t('console.status.commandHint')}
        aria-label={t('console.status.commandHint')}
      >
        <kbd className="console-kbd">⌘K</kbd>
        <span>{t('console.status.commandHint')}</span>
      </button>
    </footer>
  )
}