/**
 * Skills section: the discovery config (switch + extra roots) plus a read-only
 * listing with the validation errors verbatim.
 *
 * Skills are discovered from disk roots. The config is hot on the runtime side
 * (the runtime re-reads it on every scan), so a save takes effect without a
 * restart; a rejected patch is still rendered, never swallowed.
 */
import { useEffect, useState } from 'react'
import { Button, EmptyState, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import type { ApplyResult, SkillsSettings } from '../../../shared/contracts'
import { SKILLS_EXTRA_DIRS_MAX } from '../../../shared/contracts/web-only'
import { useSkills } from '../hooks/use-skills'
import type { SettingsApi } from '../hooks/use-settings'
import { describeOutcome, type Outcome } from '../api/outcome'
import { CheckField, Field } from './Field'
import { LoadError, Notice, SettingsSection, Skeleton } from './SectionShell'

function parseExtraDirs(input: string): string[] {
  const seen = new Set<string>()
  for (const line of input.split('\n')) {
    const dir = line.trim()
    if (dir.length === 0) continue
    seen.add(dir)
  }
  return [...seen]
}

/** The editable half: master switch plus extra scan roots. */
function SkillsConfigCard({ settings }: { settings: SettingsApi }): JSX.Element | null {
  const { t } = useI18n()
  const toast = useToast()
  const { config, saving, save } = settings
  const [enabled, setEnabled] = useState(true)
  const [dirsDraft, setDirsDraft] = useState('')
  const [rejection, setRejection] = useState<string | null>(null)

  useEffect(() => {
    if (!config) return
    setEnabled(config.skills.enabled)
    setDirsDraft(config.skills.extra_dirs.join('\n'))
  }, [config])

  if (!config) return null

  const parsed = parseExtraDirs(dirsDraft)
  const tooMany = parsed.length > SKILLS_EXTRA_DIRS_MAX
  const invalid = tooMany
  const busy = saving === 'skills'
  const dirty = enabled !== config.skills.enabled || dirsDraft !== config.skills.extra_dirs.join('\n')

  const persist = async (payload: SkillsSettings): Promise<void> => {
    setRejection(null)
    const outcome: Outcome<ApplyResult> = await save({ skills: payload }, 'skills')
    if (!outcome.ok) {
      toast.push('error', t('settings.skills.saveFailed'), describeOutcome(outcome))
      return
    }
    if (outcome.value.result === 'rejected') {
      setRejection(outcome.value.reason)
      toast.push('error', t('settings.skills.rejected'), outcome.value.reason)
      return
    }
    toast.push('success', t('settings.skills.saveSucceeded'), t('settings.skills.effectiveHint'))
  }

  return (
    <div className="ark-settings__stack">
      <div className="ark-card__foot">
        <CheckField
          label={t('settings.skills.configEnabled')}
          checked={enabled}
          disabled={busy}
          onChange={(next) => {
            setEnabled(next)
            void persist({ enabled: next, extra_dirs: parsed })
          }}
        />
        <span className="ark-settings__hint">{t('settings.skills.configEnabledHint')}</span>
      </div>

      <Field
        label={t('settings.skills.extraDirs')}
        htmlFor="skills-extra-dirs"
        hint={t('settings.skills.extraDirsHint', { max: SKILLS_EXTRA_DIRS_MAX })}
      >
        <textarea
          id="skills-extra-dirs"
          rows={3}
          spellCheck={false}
          value={dirsDraft}
          disabled={busy}
          placeholder={t('settings.skills.extraDirsPlaceholder')}
          onChange={(event) => setDirsDraft(event.target.value)}
        />
      </Field>
      {tooMany ? (
        <p className="ark-settings__hint ark-settings__hint--error">
          {t('settings.skills.extraDirsTooMany', { max: SKILLS_EXTRA_DIRS_MAX })}
        </p>
      ) : null}

      <div className="ark-card__actions">
        <Button
          variant="primary"
          size="sm"
          disabled={busy || invalid || !dirty}
          onClick={() => void persist({ enabled, extra_dirs: parsed })}
        >
          {busy ? t('common.saving') : t('common.save')}
        </Button>
      </div>
      <p className="ark-settings__hint">{t('settings.skills.effectiveHint')}</p>

      {rejection ? (
        <Notice tone="error" title={t('settings.skills.rejectedTitle')}>
          <span>{t('settings.skills.reason', { reason: rejection })}</span>
        </Notice>
      ) : null}
    </div>
  )
}

export function SkillsSection({ settings }: { settings: SettingsApi }): JSX.Element {
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
      <SkillsConfigCard settings={settings} />

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
