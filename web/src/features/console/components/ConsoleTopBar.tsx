/**
 * Global top bar for the ARK Console shell.
 *
 * One row of orientation facts the old chromeless workbench hid inside the
 * sidebar footer or the settings page: workspace, Git head, runtime state,
 * theme, settings and the ⌘K entry point. Data comes from `useConsoleMeta`
 * (workspace/diff/providers) and the session store; nothing is invented here.
 * The model chip is deliberately absent: the loop's model is only ever
 * displayed where it is selectable (composer pill / settings), and the top bar
 * shows the runtime's default-provider model, which invites misreads.
 *
 * The right-cluster gear is view-aware: from the workbench it opens settings,
 * from the settings scene it flips back to the code workbench, so the same
 * button is always an exit, never a no-op self-link.
 */
import { useI18n } from '../../../shared/i18n'
import { Icon } from '../../../shared/ui'
import { useCodeSessionStore } from '../../code-session/state/store'
import { useViewStore } from '../../workbench/state/view-slice'
import type { ConsoleMeta } from '../hooks/use-console-meta'

function workspaceName(path: string | null): string | null {
  if (!path) return null
  const parts = path.split(/[\\/]/).filter(Boolean)
  return parts.length > 0 ? (parts[parts.length - 1] ?? path) : path
}

export function ConsoleTopBar({ meta }: { meta: ConsoleMeta }): JSX.Element {
  const { t, theme, setTheme } = useI18n()
  const sending = useCodeSessionStore((s) => s.sending)
  const activeTurnId = useCodeSessionStore((s) => s.activeTurnId)
  const setView = useViewStore((s) => s.setView)
  const view = useViewStore((s) => s.view)
  const toggleCommand = useViewStore((s) => s.toggleCommand)

  const running = sending || Boolean(activeTurnId)
  const name = workspaceName(meta.workspacePath)
  const nextThemeLabel = theme === 'dark' ? t('console.top.theme') : t('console.top.theme')
  // The gear becomes the exit: once the settings scene owns the stage the only
  // same-button affordance is "back to workbench", so the icon never dead-ends
  // (opening settings from settings would be a no-op and read as broken).
  const onSettings = view === 'settings'

  return (
    <header className="console-topbar">
      <div className="console-topbar__brand">
        {/* Wordmark only: the letter tile and the text merged into one
            green-badge "ark" so the corner carries a single brand element. */}
        <span className="console-mark console-mark--word">ark</span>
      </div>

      <button
        type="button"
        className="console-chip"
        onClick={() => setView('settings')}
        title={meta.workspacePath ?? t('console.top.workspace')}
        aria-label={t('console.top.workspace')}
      >
        <span className="console-chip__icon"><Icon name="folder" size={13} /></span>
        <span className="console-chip__path">{name ?? t('console.top.workspace')}</span>
      </button>

      {meta.headSha ? (
        <span className="console-chip" title={meta.headSha}>
          <span className="console-chip__icon"><Icon name="git" size={13} /></span>
          <span className="console-chip__path">{meta.headSha.slice(0, 7)}</span>
        </span>
      ) : null}

      <div className="console-topbar__spacer" />

      <button
        type="button"
        className="console-search"
        onClick={toggleCommand}
        aria-label={t('console.top.openCommand')}
        title={t('console.top.openCommand')}
      >
        <Icon name="search" size={14} />
        <span className="console-search__label">{t('console.top.search')}</span>
        <span className="console-search__keys">
          <kbd className="console-kbd">⌘</kbd>
          <kbd className="console-kbd">K</kbd>
        </span>
      </button>

      <div className="console-topbar__spacer" />

      <div className="console-topbar__right">
        <span className="console-run-pill" role="status" aria-live="polite">
          <span className={`console-dot${running ? ' console-dot--live' : ''}`} aria-hidden="true" />
          {running ? t('console.top.running') : t('console.top.idle')}
        </span>

        <span className="console-divider" aria-hidden="true" />

        <button
          type="button"
          className="console-iconbtn"
          onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}
          aria-label={nextThemeLabel}
          title={nextThemeLabel}
        >
          <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={15} />
        </button>

        <button
          type="button"
          className="console-iconbtn"
          onClick={() => setView(onSettings ? 'code' : 'settings')}
          aria-label={onSettings ? t('console.top.backToWorkbench') : t('console.top.settings')}
          title={onSettings ? t('console.top.backToWorkbench') : t('console.top.settings')}
        >
          <Icon name={onSettings ? 'undo' : 'settings'} size={15} />
        </button>
      </div>
    </header>
  )
}