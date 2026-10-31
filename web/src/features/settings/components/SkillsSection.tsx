/**
 * Skills section: read-only listing plus the validation errors verbatim.
 *
 * Skills are discovered from disk roots; there is nothing to edit here, so the
 * section reports `enabled`, `roots`, each skill and every load failure.
 */
import { Button, EmptyState } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { useSkills } from '../hooks/use-skills'
import { LoadError, Notice, SettingsSection, Skeleton } from './SectionShell'

export function SkillsSection(): JSX.Element {
  const { t } = useI18n()
  const api = useSkills()

  const body = ((): JSX.Element => {
    if (api.status === 'loading' || api.status === 'idle') return <Skeleton rows={3} />
    if (api.status === 'error') return <LoadError message={api.error ?? t('settings.skills.loadFailed')} onRetry={() => void api.refresh()} />
    if (!api.enabled) {
      return <EmptyState title={t('settings.skills.disabledTitle')} hint={t('settings.skills.disabledHint')} />
    }
    if (api.skills.length === 0) {
      return <EmptyState
        title={t('settings.skills.emptyTitle')}
        hint={api.roots.length > 0 ? t('settings.skills.emptyHintRoots', { count: api.roots.length }) : t('settings.skills.emptyHintNoRoots')}
      />
    }
    return (
      <div className="ark-settings__list">
        {api.skills.map((skill) => (
          <div key={`${skill.root}::${skill.id}`} className="ark-card">
            <div className="ark-card__head">
              <span className="ark-card__name">{skill.name}</span>
              <span className="ark-chip">v{skill.version}</span>
              {skill.legacy ? <span className="ark-chip ark-chip--muted">legacy</span> : null}
              <span className="ark-chip ark-chip--live">{skill.source}</span>
              <span className="ark-chip ark-chip--muted">priority {skill.priority}</span>
            </div>
            {skill.description ? <p className="ark-settings__hint">{skill.description}</p> : null}
            <div className="ark-card__meta">
              <span>id: {skill.id}</span>
              <span>root: {skill.root}</span>
              <span>entry: {skill.entry_path}</span>
            </div>
          </div>
        ))}
      </div>
    )
  })()

  return (
    <SettingsSection
      title={t('settings.skills.title')}
      desc={t('settings.skills.desc')}
      actions={
        <Button size="sm" disabled={api.status === 'loading'} onClick={() => void api.refresh()}>
          {api.status === 'loading' ? t('common.refreshing') : t('common.refresh')}
        </Button>
      }
    >
      {api.roots.length > 0 ? (
        <div className="ark-card__meta">
          <span>roots:</span>
          {api.roots.map((root) => (
            <code key={root} className="ark-settings__code">
              {root}
            </code>
          ))}
        </div>
      ) : null}

      {api.validationErrors.length > 0 ? (
        <Notice
          tone="error"
          title={t('settings.skills.validationFailed', { count: api.validationErrors.length })}
          items={api.validationErrors.map((row) =>
            t('settings.skills.validationRow', { root: row.root, message: row.message })
          )}
        />
      ) : null}

      {body}
    </SettingsSection>
  )
}
