/**
 * Provider list + create/edit form.
 *
 * The list only ever shows what the runtime returns: `has_api_key` and the
 * obfuscated `api_key_hint`. A plaintext key is never rendered, because the
 * runtime never sends one back.
 */
import { useState } from 'react'
import { Button, EmptyState, useToast } from '../../../shared/ui'
import type { ProviderEntry, ProviderTestResult, ProviderView } from '../../../shared/contracts'
import type { ProvidersApi } from '../hooks/use-providers'
import { describeOutcome } from '../api/outcome'
import { draftFromView, emptyDraft, ProviderForm, type ProviderDraft } from './ProviderForm'
import { LoadError, Notice, SettingsSection, Skeleton } from './SectionShell'

/** Pending test outcome, keyed by provider id (`*` = unsaved draft). */
type TestState = { key: string; result?: ProviderTestResult; error?: string }

function ProviderRow({
  provider,
  api,
  onEdit,
  onTest,
  testState
}: {
  provider: ProviderView
  api: ProvidersApi
  onEdit: () => void
  onTest: () => void
  testState: TestState | null
}): JSX.Element {
  const toast = useToast()
  const active = api.activeProviderId === provider.id
  const isDefault = api.defaultProviderId === provider.id || provider.is_default

  const remove = async (): Promise<void> => {
    const outcome = await api.remove(provider.id)
    if (outcome.ok) toast.push('success', `已删除供应商 ${provider.id}`)
    else toast.push('error', '删除失败', describeOutcome(outcome))
  }

  const makeDefault = async (): Promise<void> => {
    const outcome = await api.setDefault(provider.id)
    if (outcome.ok) toast.push('success', `${provider.id} 已设为默认`)
    else toast.push('error', '设为默认失败', describeOutcome(outcome))
  }

  const shown = testState && testState.key === `test:${provider.id}` ? testState : null

  return (
    <div className="ark-card">
      <div className="ark-card__head">
        <span className="ark-card__name">{provider.id}</span>
        {provider.label ? <span className="ark-settings__hint">{provider.label}</span> : null}
        {isDefault ? <span className="ark-chip ark-chip--ok">默认</span> : null}
        {active ? <span className="ark-chip ark-chip--live">active</span> : null}
        {provider.enabled ? null : <span className="ark-chip ark-chip--muted">已禁用</span>}
        <div className="ark-card__actions">
          <Button size="sm" disabled={api.busy !== null || isDefault} onClick={() => void makeDefault()}>
            {isDefault ? '已是默认' : '设为默认'}
          </Button>
          <Button size="sm" disabled={api.busy !== null} onClick={onTest}>
            测试连接
          </Button>
          <Button size="sm" disabled={api.busy !== null} onClick={onEdit}>
            编辑
          </Button>
          <Button variant="danger" size="sm" disabled={api.busy !== null} onClick={() => void remove()}>
            删除
          </Button>
        </div>
      </div>

      <div className="ark-card__meta">
        <span>model: {provider.model}</span>
        <span>format: {provider.endpoint_format}</span>
        <span>base_url: {provider.base_url}</span>
        <span>
          api_key: {provider.has_api_key ? (provider.api_key_hint ? `已配置（${provider.api_key_hint}）` : '已配置') : '未配置'}
        </span>
      </div>

      {shown?.error ? <Notice tone="error" title="测试请求失败">{shown.error}</Notice> : null}
      {shown?.result ? (
        <Notice tone={shown.result.ok ? 'warn' : 'error'} title={shown.result.ok ? '测试通过' : '测试未通过'}>
          <span className="ark-card__meta">
            <span>ok: {String(shown.result.ok)}</span>
            <span>status: {shown.result.status ?? '—'}</span>
            <span>latency: {shown.result.latency_ms} ms</span>
          </span>
          <span>{shown.result.message}</span>
          {shown.result.request_url ? <code className="ark-settings__code">{shown.result.request_url}</code> : null}
        </Notice>
      ) : null}
    </div>
  )
}

export type ProvidersSectionProps = { providers: ProvidersApi }

export function ProvidersSection({ providers: api }: ProvidersSectionProps): JSX.Element {
  const toast = useToast()
  const [draft, setDraft] = useState<ProviderDraft | null>(null)
  const [editing, setEditing] = useState(false)
  const [testState, setTestState] = useState<TestState | null>(null)

  const close = (): void => {
    setDraft(null)
    setEditing(false)
  }

  const submit = async (entry: ProviderEntry, makeDefault: boolean): Promise<void> => {
    const outcome = await api.upsert({ ...entry, make_default: makeDefault })
    if (outcome.ok) {
      toast.push('success', `供应商 ${entry.id} 已保存`, makeDefault ? '并已设为默认' : undefined)
      close()
      return
    }
    toast.push('error', '保存供应商失败', describeOutcome(outcome))
  }

  const runTest = async (request: { provider_id?: string; entry?: ProviderEntry }, key: string): Promise<void> => {
    setTestState({ key })
    const outcome = await api.test(request)
    if (outcome.ok) {
      setTestState({ key, result: outcome.value })
      if (!outcome.value.ok) toast.push('error', '连接测试未通过', outcome.value.message)
      else toast.push('success', '连接测试通过', `${outcome.value.latency_ms} ms`)
      return
    }
    setTestState({ key, error: describeOutcome(outcome) })
    toast.push('error', '测试请求失败', describeOutcome(outcome))
  }

  const body = ((): JSX.Element => {
    if ((api.status === 'loading' || api.status === 'idle') && api.providers.length === 0) return <Skeleton rows={3} />
    if (api.status === 'error') return <LoadError message={api.error ?? '加载供应商失败'} onRetry={() => void api.refresh()} />
    if (api.providers.length === 0) {
      return <EmptyState title="还没有供应商" hint="至少需要一个 provider 才能发起模型请求。" />
    }
    return (
      <div className="ark-settings__list">
        {api.providers.map((provider) => (
          <ProviderRow
            key={provider.id}
            provider={provider}
            api={api}
            testState={testState}
            onEdit={() => {
              setEditing(true)
              setDraft(draftFromView(provider))
            }}
            onTest={() => void runTest({ provider_id: provider.id }, `test:${provider.id}`)}
          />
        ))}
      </div>
    )
  })()

  return (
    <SettingsSection
      title="供应商 / 模型"
      desc="provider 配置与连接测试；api_key 只写不回显。"
      actions={
        draft ? null : (
          <Button
            variant="primary"
            size="sm"
            disabled={api.busy !== null}
            onClick={() => {
              setEditing(false)
              setDraft(emptyDraft())
            }}
          >
            新增供应商
          </Button>
        )
      }
    >
      {api.defaultProviderId !== api.activeProviderId && api.activeProviderId ? (
        <p className="ark-settings__hint ark-settings__hint--warn">
          default_provider_id（{api.defaultProviderId ?? '未设置'}）与 active_provider_id（{api.activeProviderId}）不同：
          active 才是当前真正在用的供应商。
        </p>
      ) : null}

      {body}

      {draft ? (
        <ProviderForm
          key={`${editing ? 'edit' : 'create'}:${draft.id || 'new'}`}
          editing={editing}
          initial={draft}
          busy={api.busy !== null}
          onCancel={close}
          onSubmit={submit}
          onTest={(entry) => runTest({ entry }, `test:${entry.id}`)}
        />
      ) : null}
    </SettingsSection>
  )
}
