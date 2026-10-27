/**
 * Subagent section: `subagents.enabled` + `subagents.max_concurrency`.
 *
 * The runtime hard-limits concurrency to 1–8; the input is clamped here too so
 * the user sees the limit instead of a rejected save. A `rejected` answer is
 * rendered verbatim — `restart_required` is never swallowed.
 */
import { useEffect, useState } from 'react'
import { Button, useToast } from '../../../shared/ui'
import { SUBAGENT_MAX_CONCURRENCY, SUBAGENT_MIN_CONCURRENCY, type ConfigApplyResult, type SubagentConfig } from '../../../shared/contracts'
import type { SettingsApi } from '../hooks/use-settings'
import { describeOutcome, type Outcome } from '../api/outcome'
import { CheckField, Field } from './Field'
import { Notice, SettingsSection, Skeleton } from './SectionShell'

function clampConcurrency(value: number): number {
  if (!Number.isFinite(value)) return SUBAGENT_MIN_CONCURRENCY
  return Math.min(SUBAGENT_MAX_CONCURRENCY, Math.max(SUBAGENT_MIN_CONCURRENCY, Math.round(value)))
}

export function SubagentSection({ settings }: { settings: SettingsApi }): JSX.Element {
  const toast = useToast()
  const { config, saving, save } = settings
  const [enabled, setEnabled] = useState(false)
  const [concurrency, setConcurrency] = useState('2')
  const [rejection, setRejection] = useState<{ reason: string; restartRequired: string[] } | null>(null)

  // Adopt runtime values whenever the loaded config changes (first load and
  // after each successful save), but never fight an in-flight edit otherwise.
  useEffect(() => {
    if (!config) return
    setEnabled(config.subagents.enabled)
    setConcurrency(String(config.subagents.max_concurrency))
  }, [config])

  if (!config) {
    return (
      <SettingsSection title="子 agent" desc="并行子 agent 的开关与并发上限。">
        <Skeleton rows={2} />
      </SettingsSection>
    )
  }

  const parsed = Number(concurrency)
  const invalid = !Number.isInteger(parsed) || parsed < SUBAGENT_MIN_CONCURRENCY || parsed > SUBAGENT_MAX_CONCURRENCY
  const busy = saving === 'subagents'
  const dirty =
    enabled !== config.subagents.enabled || parsed !== config.subagents.max_concurrency || invalid

  const persist = async (payload: SubagentConfig, label: string): Promise<void> => {
    setRejection(null)
    const outcome: Outcome<ConfigApplyResult> = await save({ subagents: payload }, 'subagents')
    if (!outcome.ok) {
      toast.push('error', `${label}失败`, describeOutcome(outcome))
      return
    }
    if (outcome.value.result === 'rejected') {
      // Show the runtime's own words, both fields, without truncation.
      setRejection({ reason: outcome.value.reason, restartRequired: outcome.value.restart_required })
      toast.push('error', '运行时拒绝了子 agent 配置', outcome.value.reason)
      return
    }
    toast.push('success', `${label}成功`, `已变更：${outcome.value.changed.join('、') || '无'}`)
  }

  const toggleEnabled = async (next: boolean): Promise<void> => {
    setEnabled(next)
    await persist({ enabled: next, max_concurrency: clampConcurrency(Number(concurrency)) }, next ? '启动子 agent ' : '关闭子 agent ')
  }

  return (
    <SettingsSection title="子 agent" desc="并行子 agent 的开关与并发上限。">
      <div className="ark-card__foot">
        <CheckField label="启用子 agent（subagents.enabled）" checked={enabled} disabled={busy} onChange={(next) => void toggleEnabled(next)} />
        <span className="ark-settings__hint">开关切换即保存。</span>
      </div>

      <Field
        label="最大并发（subagents.max_concurrency）"
        htmlFor="subagent-concurrency"
        hint={`取值范围 ${SUBAGENT_MIN_CONCURRENCY}–${SUBAGENT_MAX_CONCURRENCY}（后端硬限制）。`}
      >
        <div className="ark-field__inline">
          <input
            id="subagent-concurrency"
            className="ark-field__input"
            type="number"
            min={SUBAGENT_MIN_CONCURRENCY}
            max={SUBAGENT_MAX_CONCURRENCY}
            step={1}
            value={concurrency}
            disabled={busy}
            onChange={(event) => setConcurrency(event.target.value)}
            onBlur={() => {
              if (invalid) setConcurrency(String(config.subagents.max_concurrency))
            }}
          />
          <Button
            variant="primary"
            size="sm"
            disabled={busy || invalid || !dirty}
            onClick={() => void persist({ enabled, max_concurrency: clampConcurrency(parsed) }, '保存并发数')}
          >
            {busy ? '保存中…' : '保存'}
          </Button>
        </div>
      </Field>

      {invalid ? (
        <p className="ark-settings__hint ark-settings__hint--error">
          并发数必须是 {SUBAGENT_MIN_CONCURRENCY}–{SUBAGENT_MAX_CONCURRENCY} 之间的整数。
        </p>
      ) : null}

      {rejection ? (
        <Notice tone="error" title="保存被拒绝（result: rejected）">
          <span>reason: {rejection.reason}</span>
          {rejection.restartRequired.length > 0 ? (
            <span>restart_required: {rejection.restartRequired.join('、')}</span>
          ) : (
            <span>restart_required: （无）</span>
          )}
        </Notice>
      ) : null}

      <p className="ark-settings__hint">
        当前运行时值：enabled = {String(config.subagents.enabled)}，max_concurrency = {config.subagents.max_concurrency}。
      </p>
    </SettingsSection>
  )
}
