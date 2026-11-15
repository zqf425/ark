import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../../code-session/state/store'
import { ChildRunsPanel } from '../../code-session/components/ChildRunsPanel'

/**
 * The inspector's subagent tab: the same ChildRunsPanel the parent stream used,
 * hosted as a resident side column. Claude-style placement — the conversation
 * keeps flowing while delegated work and its live trace live one tab away.
 */
export function SubagentsPanel(): JSX.Element {
  const { t } = useI18n()
  const threadId = useCodeSessionStore((state) => state.activeThreadId)

  if (!threadId) return <p className="ark-panel__hint">{t('workbench.subagents.noThread')}</p>
  return <ChildRunsPanel threadId={threadId} />
}
