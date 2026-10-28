/**
 * Provider create/edit form.
 *
 * Pure form state: it never calls the api itself. The parent owns `busy` and
 * folds the returned `ProviderEntry` into a request. The API key field is
 * write-only — the runtime never returns it, so leaving it blank on edit means
 * "keep the stored key".
 */
import { useState } from 'react'
import { Button } from '../../../shared/ui'
import {
  ENDPOINT_FORMATS,
  type EndpointFormat,
  type ProviderEntry,
  type ProviderModelCatalogRequest,
  type ProviderModelCatalogResponse,
  type ProviderModelSummary,
  type ProviderView
} from '../../../shared/contracts'
import { CheckField, Field, SelectField, TextField } from './Field'
import { applyCatalogModel, buildCatalogRequest, catalogLabel, parseProviderHeaders } from './ProviderCatalog'
import { Notice } from './SectionShell'

const FORMAT_LABELS: Record<EndpointFormat, string> = {
  openai_chat: 'openai_chat（/chat/completions）',
  openai_responses: 'openai_responses（/responses）',
  anthropic_messages: 'anthropic_messages（/v1/messages）'
}

export type ProviderDraft = {
  id: string
  label: string
  base_url: string
  endpoint_format: EndpointFormat
  model: string
  api_key: string
  enabled: boolean
  headers_text: string
  max_tokens: string
  temperature: string
  context_window_tokens: string
  supports_vision: boolean | null
}

export function emptyDraft(): ProviderDraft {
  return {
    id: '',
    label: '',
    base_url: '',
    endpoint_format: 'openai_chat',
    model: '',
    api_key: '',
    enabled: true,
    headers_text: '',
    max_tokens: '',
    temperature: '',
    context_window_tokens: '',
    supports_vision: null
  }
}

export function draftFromView(view: ProviderView): ProviderDraft {
  const format = (ENDPOINT_FORMATS as readonly string[]).includes(view.endpoint_format)
    ? (view.endpoint_format as EndpointFormat)
    : 'openai_chat'
  return {
    id: view.id,
    label: view.label ?? '',
    base_url: view.base_url,
    endpoint_format: format,
    model: view.model,
    // Never prefilled: the runtime only ever reports a hint, not the key.
    api_key: '',
    enabled: view.enabled,
    headers_text: '',
    max_tokens: '',
    temperature: '',
    context_window_tokens: view.context_window_tokens?.toString() ?? '',
    supports_vision: view.supports_vision ?? null
  }
}

export type BuiltEntry = { ok: true; entry: ProviderEntry } | { ok: false; error: string }

/** Validate locally, then map onto the wire shape. Numbers stay strings in the form. */
export function buildEntry(draft: ProviderDraft, editing: boolean): BuiltEntry {
  const id = draft.id.trim()
  const baseUrl = draft.base_url.trim()
  const model = draft.model.trim()
  if (!id) return { ok: false, error: 'id 不能为空' }
  if (!baseUrl) return { ok: false, error: 'base_url 不能为空' }
  if (!model) return { ok: false, error: 'model 不能为空' }

  const entry: ProviderEntry = {
    id,
    base_url: baseUrl,
    endpoint_format: draft.endpoint_format,
    model,
    enabled: draft.enabled,
    headers: {}
  }
  const label = draft.label.trim()
  if (label) entry.label = label

  const apiKey = draft.api_key.trim()
  if (apiKey) entry.api_key = apiKey
  else if (!editing) return { ok: false, error: '新建供应商需要填写 api_key' }

  const headers = parseProviderHeaders(draft.headers_text)
  if (!headers.ok) return headers
  entry.headers = headers.headers

  if (draft.max_tokens.trim()) {
    const parsed = Number(draft.max_tokens.trim())
    if (!Number.isFinite(parsed) || parsed <= 0 || !Number.isInteger(parsed)) {
      return { ok: false, error: 'max_tokens 必须是正整数' }
    }
    entry.max_tokens = parsed
  }

  if (draft.temperature.trim()) {
    const parsed = Number(draft.temperature.trim())
    if (!Number.isFinite(parsed) || parsed < 0 || parsed > 2) {
      return { ok: false, error: 'temperature 需在 0–2 之间' }
    }
    entry.temperature = parsed
  }

  if (draft.context_window_tokens.trim()) {
    const parsed = Number(draft.context_window_tokens.trim())
    if (!Number.isSafeInteger(parsed) || parsed <= 0) {
      return { ok: false, error: 'context_window_tokens 必须是正整数' }
    }
    entry.context_window_tokens = parsed
  }
  if (draft.supports_vision !== null) entry.supports_vision = draft.supports_vision

  return { ok: true, entry }
}

export type ProviderFormProps = {
  editing: boolean
  initial: ProviderDraft
  busy: boolean
  onCancel: () => void
  onSubmit: (entry: ProviderEntry, makeDefault: boolean) => Promise<void>
  onTest: (entry: ProviderEntry) => Promise<void>
  onDiscover: (request: ProviderModelCatalogRequest) => Promise<ProviderModelCatalogResponse>
}

export function ProviderForm({ editing, initial, busy, onCancel, onSubmit, onTest, onDiscover }: ProviderFormProps): JSX.Element {
  const [draft, setDraft] = useState<ProviderDraft>(initial)
  const [advanced, setAdvanced] = useState(false)
  const [makeDefault, setMakeDefault] = useState(!editing)
  const [error, setError] = useState<string | null>(null)
  const [catalog, setCatalog] = useState<ProviderModelSummary[]>([])
  const [catalogError, setCatalogError] = useState<string | null>(null)

  const patch = <K extends keyof ProviderDraft>(key: K, value: ProviderDraft[K]): void => {
    setDraft((current) => ({ ...current, [key]: value }))
  }

  const resolve = (): ProviderEntry | null => {
    const built = buildEntry(draft, editing)
    if (!built.ok) {
      setError(built.error)
      return null
    }
    setError(null)
    return built.entry
  }

  const discover = async (): Promise<void> => {
    const built = buildCatalogRequest(draft, editing)
    if (!built.ok) {
      setCatalogError(built.error)
      return
    }
    setCatalogError(null)
    try {
      const response = await onDiscover(built.request)
      setCatalog(response.models)
      if (response.models.length === 0) setCatalogError('供应商没有返回可选择的模型。')
    } catch (thrown) {
      setCatalog([])
      setCatalogError(thrown instanceof Error ? thrown.message : '查询模型失败')
    }
  }

  const catalogLabels = Object.fromEntries(catalog.map((model) => [model.id, catalogLabel(model)])) as Record<string, string>

  return (
    <div className="ark-card ark-card--editing">
      <div className="ark-card__meta">
        <strong>{editing ? `编辑供应商 ${initial.id}` : '新增供应商'}</strong>
      </div>

      <div className="ark-settings__grid">
        <TextField
          id="provider-id"
          label="id"
          value={draft.id}
          mono
          disabled={editing || busy}
          placeholder="deepseek"
          hint={editing ? 'id 是主键，编辑时不可修改。' : '唯一标识，保存后用于 default provider。'}
          onChange={(value) => patch('id', value)}
        />
        <TextField
          id="provider-label"
          label="label（可选）"
          value={draft.label}
          disabled={busy}
          placeholder="DeepSeek 官方"
          onChange={(value) => patch('label', value)}
        />
        <TextField
          id="provider-base-url"
          label="base_url"
          value={draft.base_url}
          mono
          disabled={busy}
          placeholder="https://api.deepseek.com"
          onChange={(value) => patch('base_url', value)}
        />
        <SelectField<EndpointFormat>
          id="provider-format"
          label="endpoint_format"
          value={draft.endpoint_format}
          options={ENDPOINT_FORMATS}
          labels={FORMAT_LABELS}
          disabled={busy}
          onChange={(value) => patch('endpoint_format', value)}
        />
        <TextField
          id="provider-model"
          label="model"
          value={draft.model}
          mono
          disabled={busy}
          placeholder="deepseek-chat"
          onChange={(value) => patch('model', value)}
        />
        {catalog.length > 0 ? (
          <Field
            label="供应商模型目录"
            htmlFor="provider-catalog-model"
            hint="选择后自动填入目录明确声明的上下文窗口和识图能力；未声明的能力保持未知。"
          >
            <select
              id="provider-catalog-model"
              className="ark-field__select"
              value={catalog.some((item) => item.id === draft.model) ? draft.model : ''}
              disabled={busy}
              onChange={(event) => {
                const model = catalog.find((item) => item.id === event.target.value)
                if (model) setDraft((current) => applyCatalogModel(current, model))
              }}
            >
              <option value="">选择模型后再写入草稿</option>
              {catalog.map((model) => <option key={model.id} value={model.id}>{catalogLabels[model.id] ?? model.id}</option>)}
            </select>
          </Field>
        ) : null}
        <TextField
          id="provider-api-key"
          label="api_key"
          type="password"
          value={draft.api_key}
          disabled={busy}
          placeholder={editing ? '留空 = 保留原 key' : 'sk-…'}
          hint={editing ? '编辑时留空表示保留已存的 key，不会覆盖。' : '仅写入运行时，列表只回显 has_api_key 与 hint。'}
          onChange={(value) => patch('api_key', value)}
        />
      </div>

      <div className="ark-card__foot">
        <CheckField label="启用（enabled）" checked={draft.enabled} disabled={busy} onChange={(value) => patch('enabled', value)} />
        {!editing ? (
          <CheckField label="保存后设为默认" checked={makeDefault} disabled={busy} onChange={setMakeDefault} />
        ) : null}
        <button type="button" className="ark-linkbtn" onClick={() => setAdvanced((value) => !value)}>
          {advanced ? '收起高级选项' : '展开高级选项（headers / max_tokens / temperature）'}
        </button>
        <Button size="sm" disabled={busy} onClick={() => void discover()}>
          {busy ? '查询中…' : '查询模型列表'}
        </Button>
      </div>

      {advanced ? (
        <div className="ark-settings__grid">
          <Field
            label="headers（JSON 对象，可选）"
            htmlFor="provider-headers"
            hint='例如 {"X-Request-Source":"ark"}；留空则不发送额外头。'
          >
            <textarea
              id="provider-headers"
              className="ark-field__area"
              value={draft.headers_text}
              disabled={busy}
              spellCheck={false}
              placeholder={'{\n  "X-Request-Source": "ark"\n}'}
              onChange={(event) => patch('headers_text', event.target.value)}
            />
          </Field>
          <TextField
            id="provider-max-tokens"
            label="max_tokens（可选）"
            value={draft.max_tokens}
            disabled={busy}
            placeholder="4096"
            type="number"
            hint="留空则使用运行时默认值。"
            onChange={(value) => patch('max_tokens', value)}
          />
          <TextField
            id="provider-temperature"
            label="temperature（可选，0–2）"
            value={draft.temperature}
            disabled={busy}
            placeholder="0.3"
            type="number"
            hint="留空则使用运行时默认值。"
            onChange={(value) => patch('temperature', value)}
          />
          <TextField
            id="provider-context-window"
            label="context_window_tokens（可选）"
            value={draft.context_window_tokens}
            disabled={busy}
            placeholder="128000"
            type="number"
            hint="模型目录有明确数据时会自动填写；运行时会用它限制上下文压缩阈值。"
            onChange={(value) => patch('context_window_tokens', value)}
          />
          <CheckField
            label="支持识图"
            checked={draft.supports_vision === true}
            disabled={busy}
            onChange={(value) => patch('supports_vision', value)}
          />
        </div>
      ) : null}

      {error ? <Notice tone="error" title="表单校验失败">{error}</Notice> : null}
      {catalogError ? <Notice tone="error" title="模型目录查询失败">{catalogError}</Notice> : null}

      <div className="ark-settings__actions">
        <Button
          variant="primary"
          size="sm"
          disabled={busy}
          onClick={() => {
            const entry = resolve()
            if (entry) void onSubmit(entry, makeDefault)
          }}
        >
          {busy ? '保存中…' : '保存'}
        </Button>
        <Button
          size="sm"
          disabled={busy}
          onClick={() => {
            const entry = resolve()
            if (entry) void onTest(entry)
          }}
        >
          测试连接
        </Button>
        <Button variant="ghost" size="sm" disabled={busy} onClick={onCancel}>
          取消
        </Button>
      </div>
    </div>
  )
}
