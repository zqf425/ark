/**
 * MCP section: list, add (stdio / http), delete, test.
 *
 * `transport` decides which fields matter, so the form shows only those. Test
 * output (tool list + latency) is per row and stays until the next test.
 */
import { useState } from 'react'
import { Button, EmptyState, useToast } from '../../../shared/ui'
import type { McpServerUpsertRequest, McpServerView, McpServerTestResponse, McpTransportKind } from '../../../shared/contracts'
import { useMcpServers } from '../hooks/use-mcp'
import { describeOutcome } from '../api/outcome'
import { CheckField, Field, SelectField, TextField } from './Field'
import { LoadError, Notice, SettingsSection, Skeleton } from './SectionShell'

const TRANSPORT_LABELS: Record<McpTransportKind, string> = {
  stdio: 'stdio（本地进程）',
  sse: 'sse（HTTP 事件流）',
  streamable_http: 'streamable_http'
}

type TestState = { serverId: string; result?: McpServerTestResponse; error?: string }

function ServerRow({
  server,
  busy,
  testState,
  onTest,
  onEdit,
  onDelete
}: {
  server: McpServerView
  busy: boolean
  testState: TestState | null
  onTest: () => void
  onEdit: () => void
  onDelete: () => void
}): JSX.Element {
  const shown = testState && testState.serverId === server.id ? testState : null
  return (
    <div className="ark-card">
      <div className="ark-card__head">
        <span className="ark-card__name">{server.id}</span>
        <span className="ark-chip">{server.transport}</span>
        {server.connected ? <span className="ark-chip ark-chip--ok">已连接</span> : <span className="ark-chip ark-chip--muted">未连接</span>}
        {server.enabled ? null : <span className="ark-chip ark-chip--muted">已禁用</span>}
        <div className="ark-card__actions">
          <Button size="sm" disabled={busy} onClick={onTest}>
            测试连接
          </Button>
          <Button size="sm" disabled={busy} onClick={onEdit}>
            编辑
          </Button>
          <Button variant="danger" size="sm" disabled={busy} onClick={onDelete}>
            删除
          </Button>
        </div>
      </div>

      <div className="ark-card__meta">
        <span>tool_count: {server.tool_count}</span>
        {server.has_env ? <span>env: 已配置（编辑留空会保留）</span> : null}
        {server.has_headers ? <span>headers: 已配置（编辑留空会保留）</span> : null}
        {server.last_error ? <span className="ark-settings__hint--error">last_error: {server.last_error}</span> : null}
      </div>

      {shown?.error ? <Notice tone="error" title="测试请求失败">{shown.error}</Notice> : null}
      {shown?.result ? (
        <Notice tone={shown.result.ok ? 'warn' : 'error'} title={shown.result.ok ? '测试通过' : '测试未通过'}>
          <span className="ark-card__meta">
            <span>ok: {String(shown.result.ok)}</span>
            <span>latency: {shown.result.latency_ms} ms</span>
            <span>tools: {shown.result.tools.length}</span>
          </span>
          <span>{shown.result.message}</span>
          {shown.result.tools.length > 0 ? (
            <div className="ark-settings__tools">
              {shown.result.tools.map((tool) => (
                <span key={tool} className="ark-chip">
                  {tool}
                </span>
              ))}
            </div>
          ) : null}
        </Notice>
      ) : null}
    </div>
  )
}

type Draft = {
  id: string
  transport: McpTransportKind
  enabled: boolean
  command: string
  args: string
  url: string
  allowTools: string
  envText: string
  headersText: string
}

const EMPTY_DRAFT: Draft = { id: '', transport: 'stdio', enabled: true, command: '', args: '', url: '', allowTools: '', envText: '', headersText: '' }

function draftFromServer(server: McpServerView): Draft {
  return {
    id: server.id,
    transport: server.transport,
    enabled: server.enabled,
    command: server.command ?? '',
    args: server.args.join('\n'),
    url: server.url ?? '',
    allowTools: server.allow_tools.join('\n'),
    envText: '',
    headersText: ''
  }
}

/** Parse the args textarea: one argument per line, blank lines dropped. */
function parseArgs(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
}

function parseMap(text: string, label: string): { value?: Record<string, string>; error?: string } {
  if (!text.trim()) return { value: {} }
  try {
    const value: unknown = JSON.parse(text)
    if (!value || typeof value !== 'object' || Array.isArray(value)) return { error: `${label} 必须是 JSON 对象` }
    return { value: Object.fromEntries(Object.entries(value as Record<string, unknown>).map(([key, item]) => [key, String(item)])) }
  } catch {
    return { error: `${label} 不是合法 JSON` }
  }
}

export function McpSection(): JSX.Element {
  const toast = useToast()
  const api = useMcpServers()
  const [draft, setDraft] = useState<Draft | null>(null)
  const [editing, setEditing] = useState(false)
  const [formError, setFormError] = useState<string | null>(null)
  const [testState, setTestState] = useState<TestState | null>(null)

  const submit = async (): Promise<void> => {
    if (!draft) return
    const id = draft.id.trim()
    if (!id) {
      setFormError('id 不能为空')
      return
    }
    const isStdio = draft.transport === 'stdio'
    const command = draft.command.trim()
    const url = draft.url.trim()
    if (isStdio && !command) {
      setFormError('stdio 需要填写 command')
      return
    }
    if (!isStdio && !url) {
      setFormError('http / sse 需要填写 url')
      return
    }
    const env = parseMap(draft.envText, 'env')
    const headers = parseMap(draft.headersText, 'headers')
    if (env.error || headers.error) {
      setFormError(env.error ?? headers.error ?? '高级配置无效')
      return
    }

    const request: McpServerUpsertRequest = {
      id,
      transport: draft.transport,
      enabled: draft.enabled,
      args: [],
      env: env.value ?? {},
      headers: headers.value ?? {},
      allow_tools: parseArgs(draft.allowTools)
    }
    if (isStdio) {
      request.command = command
      const args = parseArgs(draft.args)
      if (args.length > 0) request.args = args
    } else {
      request.url = url
    }

    setFormError(null)
    const outcome = await api.add(request)
    if (outcome.ok) {
      toast.push('success', `MCP server ${id} 已保存`)
      setDraft(null)
      setEditing(false)
      return
    }
    const message = describeOutcome(outcome)
    setFormError(message)
    toast.push('error', '保存 MCP server 失败', message)
  }

  const remove = async (serverId: string): Promise<void> => {
    const outcome = await api.remove(serverId)
    if (outcome.ok) toast.push('success', `已删除 MCP server ${serverId}`)
    else toast.push('error', '删除失败', describeOutcome(outcome))
  }

  const test = async (serverId: string): Promise<void> => {
    setTestState({ serverId })
    const outcome = await api.test(serverId)
    if (outcome.ok) {
      setTestState({ serverId, result: outcome.value })
      if (outcome.value.ok) toast.push('success', 'MCP 测试通过', `${outcome.value.tools.length} 个工具 · ${outcome.value.latency_ms} ms`)
      else toast.push('error', 'MCP 测试未通过', outcome.value.message)
      void api.refresh()
      return
    }
    const message = describeOutcome(outcome)
    setTestState({ serverId, error: message })
    toast.push('error', 'MCP 测试请求失败', message)
  }

  const body = ((): JSX.Element => {
    if ((api.status === 'loading' || api.status === 'idle') && api.servers.length === 0) return <Skeleton rows={2} />
    if (api.status === 'error') return <LoadError message={api.error ?? '加载 MCP server 失败'} onRetry={() => void api.refresh()} />
    if (api.servers.length === 0) return <EmptyState title="还没有 MCP server" hint="新增一个 stdio 或 http server 后可在此测试连接。" />
    return (
      <div className="ark-settings__list">
        {api.servers.map((server) => (
          <ServerRow
            key={server.id}
            server={server}
            busy={api.busy !== null}
            testState={testState}
            onTest={() => void test(server.id)}
            onEdit={() => {
              setFormError(null)
              setEditing(true)
              setDraft(draftFromServer(server))
            }}
            onDelete={() => void remove(server.id)}
          />
        ))}
      </div>
    )
  })()

  return (
    <SettingsSection
      title="MCP servers"
      desc="外部工具服务；测试会返回工具清单与耗时。"
      actions={
        draft ? null : (
          <Button
            variant="primary"
            size="sm"
            disabled={api.busy !== null}
            onClick={() => {
              setFormError(null)
              setEditing(false)
              setDraft(EMPTY_DRAFT)
            }}
          >
            新增 server
          </Button>
        )
      }
    >
      {body}

      {draft ? (
        <div className="ark-card ark-card--editing">
          <div className="ark-card__meta">
            <strong>{editing ? `编辑 MCP server ${draft.id}` : '新增 MCP server'}</strong>
          </div>
          <div className="ark-settings__grid">
            <TextField id="mcp-id" label="id" value={draft.id} mono disabled={editing || api.busy !== null} placeholder="filesystem" onChange={(value) => setDraft({ ...draft, id: value })} />
            <SelectField<McpTransportKind>
              id="mcp-transport"
              label="transport"
              value={draft.transport}
              options={['stdio', 'sse', 'streamable_http']}
              labels={TRANSPORT_LABELS}
              disabled={api.busy !== null}
              onChange={(value) => setDraft({ ...draft, transport: value })}
            />
            {draft.transport === 'stdio' ? (
              <>
                <TextField
                  id="mcp-command"
                  label="command"
                  value={draft.command}
                  mono
                  disabled={api.busy !== null}
                  placeholder="npx"
                  onChange={(value) => setDraft({ ...draft, command: value })}
                />
                <Field label="args（每行一个参数）" htmlFor="mcp-args" hint="空行会被忽略。">
                  <textarea
                    id="mcp-args"
                    className="ark-field__area"
                    value={draft.args}
                    disabled={api.busy !== null}
                    spellCheck={false}
                    placeholder={'-y\n@modelcontextprotocol/server-filesystem\nD:\\code'}
                    onChange={(event) => setDraft({ ...draft, args: event.target.value })}
                  />
                </Field>
              </>
            ) : (
              <TextField
                id="mcp-url"
                label="url"
                value={draft.url}
                mono
                disabled={api.busy !== null}
                placeholder="http://127.0.0.1:8931/mcp"
                onChange={(value) => setDraft({ ...draft, url: value })}
              />
            )}
          </div>

          <div className="ark-card__foot">
            <CheckField label="启用（enabled）" checked={draft.enabled} disabled={api.busy !== null} onChange={(value) => setDraft({ ...draft, enabled: value })} />
          </div>

          <div className="ark-settings__grid">
            <Field label="allow_tools（每行一个；留空 = 全部）" htmlFor="mcp-allow-tools">
              <textarea id="mcp-allow-tools" className="ark-field__area" value={draft.allowTools} disabled={api.busy !== null} spellCheck={false} onChange={(event) => setDraft({ ...draft, allowTools: event.target.value })} />
            </Field>
            <Field label="env（JSON 对象，可选）" htmlFor="mcp-env" hint={editing ? '留空会保留已配置的环境变量值。' : '只写入运行时，不会在列表回显。'}>
              <textarea id="mcp-env" className="ark-field__area" value={draft.envText} disabled={api.busy !== null} spellCheck={false} placeholder={'{\n  "TOKEN": "…"\n}'} onChange={(event) => setDraft({ ...draft, envText: event.target.value })} />
            </Field>
            <Field label="headers（JSON 对象，可选）" htmlFor="mcp-headers" hint={editing ? '留空会保留已配置的 header 值。' : '只写入运行时，不会在列表回显。'}>
              <textarea id="mcp-headers" className="ark-field__area" value={draft.headersText} disabled={api.busy !== null} spellCheck={false} placeholder={'{\n  "Authorization": "Bearer …"\n}'} onChange={(event) => setDraft({ ...draft, headersText: event.target.value })} />
            </Field>
          </div>

          {formError ? <Notice tone="error" title="无法保存">{formError}</Notice> : null}

          <div className="ark-settings__actions">
            <Button variant="primary" size="sm" disabled={api.busy !== null} onClick={() => void submit()}>
              {api.busy ? '保存中…' : '保存'}
            </Button>
            <Button variant="ghost" size="sm" disabled={api.busy !== null} onClick={() => { setDraft(null); setEditing(false) }}>
              取消
            </Button>
          </div>
        </div>
      ) : null}
    </SettingsSection>
  )
}
