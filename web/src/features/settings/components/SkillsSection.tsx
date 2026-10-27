/**
 * Skills section: read-only listing plus the validation errors verbatim.
 *
 * Skills are discovered from disk roots; there is nothing to edit here, so the
 * section reports `enabled`, `roots`, each skill and every load failure.
 */
import { Button, EmptyState } from '../../../shared/ui'
import { useSkills } from '../hooks/use-skills'
import { LoadError, Notice, SettingsSection, Skeleton } from './SectionShell'

export function SkillsSection(): JSX.Element {
  const api = useSkills()

  const body = ((): JSX.Element => {
    if (api.status === 'loading' || api.status === 'idle') return <Skeleton rows={3} />
    if (api.status === 'error') return <LoadError message={api.error ?? '加载 skills 失败'} onRetry={() => void api.refresh()} />
    if (!api.enabled) {
      return <EmptyState title="skills 未启用" hint="运行时未开启 skill 发现；启用后此处会列出已加载的 skill。" />
    }
    if (api.skills.length === 0) {
      return <EmptyState title="没有发现 skill" hint={api.roots.length > 0 ? `已扫描 ${api.roots.length} 个 root。` : '未配置 skill root。'} />
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
      title="Skills"
      desc="从配置的 root 发现；只读。"
      actions={
        <Button size="sm" disabled={api.status === 'loading'} onClick={() => void api.refresh()}>
          {api.status === 'loading' ? '刷新中…' : '刷新'}
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
          title={`加载失败 ${api.validationErrors.length} 处`}
          items={api.validationErrors.map((row) => `${row.root}：${row.message}`)}
        />
      ) : null}

      {body}
    </SettingsSection>
  )
}
