/**
 * Hot turn and SSE limits.
 *
 * Values are generated from the Rust `TuningSettings` wire contract. The page
 * deliberately submits the complete typed object: config validation stays in
 * the runtime and one accepted snapshot cannot contain a mixed old/new limit.
 */
import { useEffect, useMemo, useState } from 'react'
import { Button, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import type { TuningSettings } from '../../../shared/contracts'
import type { SettingsApi } from '../hooks/use-settings'
import { describeOutcome } from '../api/outcome'
import { Field } from './Field'
import { Notice, SettingsSection, Skeleton } from './SectionShell'

type TuningKey = keyof TuningSettings

type TuningField = {
  key: TuningKey
  label: string
  hint: string
  min: number
  max: number
}

const FIELDS: readonly TuningField[] = [
  { key: 'max_rounds', label: '最大模型轮次', hint: '单回合的模型往返上限。', min: 1, max: 200 },
  { key: 'max_tokens', label: '最大累计 token', hint: '单回合的累计 token 上限。', min: 1_000, max: 50_000_000 },
  { key: 'max_tool_calls', label: '最大工具调用数', hint: '单回合工具调用上限。', min: 1, max: 5_000 },
  { key: 'max_identical_calls', label: '重复调用上限', hint: '同一调用签名达到此次数后被抑制。', min: 1, max: 100 },
  { key: 'context_limit_tokens', label: '上下文压缩阈值', hint: '预计 prompt 达到该值时触发压缩。', min: 1_000, max: 10_000_000 },
  { key: 'keep_recent_messages', label: '压缩保留消息数', hint: '压缩时原样保留的最近消息数。', min: 1, max: 10_000 },
  { key: 'chunk_buffer', label: '流式 chunk 缓冲', hint: '每轮模型增量 channel 容量。', min: 1, max: 4_096 },
  { key: 'sse_heartbeat_secs', label: 'SSE 心跳（秒）', hint: '0 会按运行时最小间隔处理。', min: 0, max: 3_600 },
  { key: 'sse_idle_timeout_secs', label: 'SSE 空闲超时（秒）', hint: '0 表示不因空闲关闭。', min: 0, max: 86_400 }
]

function cloneTuning(value: TuningSettings): TuningSettings {
  return { ...value }
}

function validDraft(draft: TuningSettings): boolean {
  return FIELDS.every(({ key, min, max }) => {
    const value = draft[key]
    return Number.isInteger(value) && value >= min && value <= max
  })
}

export function TuningSection({ settings }: { settings: SettingsApi }): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const { config, saving, save } = settings
  const [draft, setDraft] = useState<TuningSettings | null>(null)
  const [rejection, setRejection] = useState<string | null>(null)
  const [defaultAutonomy, setDefaultAutonomy] = useState('workspace_write')

  useEffect(() => {
    if (config) {
      setDraft(cloneTuning(config.tuning))
      setDefaultAutonomy(config.default_autonomy)
    }
  }, [config])

  const valid = draft !== null && validDraft(draft)
  const dirty = useMemo(() => {
    if (!config || !draft) return false
    return FIELDS.some(({ key }) => draft[key] !== config.tuning[key])
  }, [config, draft])

  if (!config || !draft) {
    return (
      <SettingsSection title="运行调优" desc="下一回合生效的模型、工具和 SSE 限制。">
        <Skeleton rows={5} />
      </SettingsSection>
    )
  }

  const setValue = (key: TuningKey, raw: string): void => {
    const next = Number(raw)
    setDraft((current) => (current ? { ...current, [key]: next } : current))
  }

  const persist = async (): Promise<void> => {
    if (!valid) return
    setRejection(null)
    const outcome = await save({ tuning: draft }, 'tuning')
    if (!outcome.ok) {
      toast.push('error', '保存运行调优失败', describeOutcome(outcome))
      return
    }
    if (outcome.value.result === 'rejected') {
      setRejection(outcome.value.reason)
      toast.push('error', '运行时拒绝了调优配置', outcome.value.reason)
      return
    }
    toast.push('success', '运行调优已保存', outcome.value.changed.join('、') || '无实际变更')
  }

  const persistDefaultAutonomy = async (): Promise<void> => {
    const outcome = await save({ default_autonomy: defaultAutonomy }, 'default_autonomy')
    if (!outcome.ok) {
      toast.push('error', t('settings.autonomy.saveFailed'), describeOutcome(outcome))
      return
    }
    if (outcome.value.result === 'rejected') {
      setRejection(outcome.value.reason)
      return
    }
    toast.push('success', t('settings.autonomy.saved'))
  }

  return (
    <SettingsSection title="运行调优" desc="配置热更新；运行中的回合保留创建时快照，下一回合使用新限制。">
      <div className="ark-settings__grid">
        {FIELDS.map((field) => {
          const value = draft[field.key]
          const invalid = !Number.isInteger(value) || value < field.min || value > field.max
          return (
            <Field key={field.key} label={field.label} htmlFor={`tuning-${field.key}`} hint={`${field.hint} 范围 ${field.min}–${field.max}。`}>
              <input
                id={`tuning-${field.key}`}
                className="ark-field__input"
                type="number"
                min={field.min}
                max={field.max}
                step={1}
                value={Number.isFinite(value) ? value : ''}
                disabled={saving === 'tuning'}
                aria-invalid={invalid}
                onChange={(event) => setValue(field.key, event.target.value)}
              />
            </Field>
          )
        })}
      </div>

      <Field label={t('settings.autonomy.label')} htmlFor="default-autonomy" hint={t('settings.autonomy.hint')}>
        <select id="default-autonomy" className="ark-field__input" value={defaultAutonomy} disabled={saving === 'default_autonomy'} onChange={(event) => setDefaultAutonomy(event.target.value)}>
          <option value="strict">{t('session.runtime.strict')}</option>
          <option value="workspace_write">{t('session.runtime.workspaceWrite')}</option>
          <option value="unattended">{t('session.runtime.unattended')}</option>
        </select>
      </Field>

      {!valid ? <Notice tone="error" title="存在无效调优值">每个值必须是对应范围内的整数。</Notice> : null}
      {rejection ? <Notice tone="error" title="保存被运行时拒绝">{rejection}</Notice> : null}

      <div className="ark-settings__actions">
        <Button variant="primary" size="sm" disabled={defaultAutonomy === config.default_autonomy || saving === 'default_autonomy'} onClick={() => void persistDefaultAutonomy()}>
          {saving === 'default_autonomy' ? t('settings.autonomy.saving') : t('settings.autonomy.save')}
        </Button>
        <Button variant="primary" size="sm" disabled={!valid || !dirty || saving === 'tuning'} onClick={() => void persist()}>
          {saving === 'tuning' ? '保存中…' : '保存调优'}
        </Button>
        <Button size="sm" disabled={!dirty || saving === 'tuning'} onClick={() => setDraft(cloneTuning(config.tuning))}>
          还原
        </Button>
      </div>
    </SettingsSection>
  )
}
