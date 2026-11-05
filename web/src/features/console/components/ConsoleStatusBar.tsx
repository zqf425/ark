/**
 * Bottom status bar: liveness, workspace, change count, context meter, usage,
 * version and the ⌘K hint. Every number comes from the runtime or the folded
 * usage aggregate — no build constants, no invented telemetry.
 */
import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../../code-session/state/store'
import { useContextLimit } from '../../code-session/hooks/use-context-limit'
import { useRuntimeStatusLabels } from '../../code-session/hooks/use-runtime-status-labels'
import { useViewStore } from '../../workbench/state/view-slice'
import type { ConsoleMeta } from '../hooks/use-console-meta'

export function ConsoleStatusBar({ meta }: { meta: ConsoleMeta }): JSX.Element {
  const { t } = useI18n()
  const { online, refreshLabel, refresh } = useRuntimeStatusLabels()
  const usage = useCodeSessionStore((s) => s.usage)
  const runtimeVersion = useCodeSessionStore((s) => s.runtimeVersion)
  const healthVersion = useCodeSessionStore((s) => s.healthVersion)
  const setCommandOpen = useViewStore((s) => s.setCommandOpen)
  const contextLimit = useContextLimit()

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

  const lastPrompt = usage?.lastPromptTokens ?? null
  const percent = lastPrompt && contextLimit ? Math.min(100, Math.round((lastPrompt / contextLimit) * 100)) : null
  const version = runtimeVersion ?? healthVersion

  return (
    <footer className="console-status">
      <button
        type="button"
        className="console-status__item console-status__ok"
        onClick={() => void refresh()}
        title={refreshLabel}
        aria-label={refreshLabel}
        style={{ border: 0, background: 'transparent', color: 'inherit', cursor: 'pointer', padding: 0 }}
      >
        <span className={`console-dot ${dotClass}`} aria-hidden="true" />
        {onlineLabelText}
      </button>

      <span aria-hidden="true">·</span>
      <span className="console-status__item console-status__item--ellipsis" title={meta.workspacePath ?? ''}>
        <b>{meta.workspacePath ?? t('console.top.workspace')}</b>
      </span>

      <span aria-hidden="true">·</span>
      <span className="console-status__item">
        {meta.changeCount > 0
          ? t('console.status.changes', { count: meta.changeCount })
          : t('console.status.changesClean')}
      </span>

      <span className="console-status__grow" />

      {percent !== null ? (
        <span className="console-status__item" title={`${lastPrompt ?? 0} / ${contextLimit ?? 0}`}>
          {t('console.status.context')}
          <span className="console-meter"><i style={{ width: `${percent}%` }} /></span>
          <b>{percent}%</b>
        </span>
      ) : null}

      {usage ? (
        <span className="console-status__item">
          {t('console.status.tokenCount', { count: usage.totalTokens })}
        </span>
      ) : null}

      <span aria-hidden="true">·</span>
      <span className="console-status__item">{version ?? '—'}</span>

      <span aria-hidden="true">·</span>
      <button
        type="button"
        className="console-status__item"
        onClick={() => setCommandOpen(true)}
        title={t('console.status.commandHint')}
        style={{ border: 0, background: 'transparent', color: 'inherit', cursor: 'pointer', padding: 0 }}
      >
        <kbd className="console-kbd">⌘K</kbd>
        {t('console.status.commandHint')}
      </button>
    </footer>
  )
}