/**
 * MCP section: list, add (stdio / http), delete, test.
 *
 * `transport` decides which fields matter, so the form shows only those. Test
 * output (tool list + latency) is per row and stays until the next test.
 */
import { useState } from 'react'
import { Button, EmptyState, useToast } from '../../../shared/ui'
import type { McpServerUpsertRequest, McpServerView, McpServerTestResponse, McpTransportKind } from '../../../shared/contracts'
import { useI18n, type MessageKey } from '../../../shared/i18n'
import { translateNow } from '../../../shared/i18n/current'
import { useMcpServers } from '../hooks/use-mcp'
import { describeOutcome } from '../api/outcome'
import { CheckField, Field, SelectField, TextField } from './Field'
import { LoadError, Notice, SettingsSection, Skeleton } from './SectionShell'

const TRANSPORT_LABEL_KEYS: Record<McpTransportKind, MessageKey> = {
  stdio: 'settings.mcp.transport.stdio',
  sse: 'settings.mcp.transport.sse',
  streamable_http: 'settings.mcp.transport.streamableHttp'
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
  const { t } = useI18n()
  const shown = testState && testState.serverId === server.id ? testState : null
  return (
    <div className="ark-card">
      <div className="ark-card__head">
        <span className="ark-card__name">{server.id}</span>
        <span className="ark-chip">{server.transport}</span>
        {server.connected ? (
          <span className="ark-chip ark-chip--ok">{t('settings.mcp.connected')}</span>
        ) : (
          <span className="ark-chip ark-chip--muted">{t('settings.mcp.disconnected')}</span>
        )}
        {server.enabled ? null : <span className="ark-chip ark-chip--muted">{t('settings.mcp.disabled')}</span>}
        <div className="ark-card__actions">
          <Button size="sm" disabled={busy} onClick={onTest}>
            {t('common.test')}
          </Button>
          <Button size="sm" disabled={busy} onClick={onEdit}>
            {t('common.edit')}
          </Button>
          <Button variant="danger" size="sm" disabled={busy} onClick={onDelete}>
            {t('common.delete')}
          </Button>
        </div>
      </div>

      <div className="ark-card__meta">
        <span>tool_count: {server.tool_count}</span>
        {server.has_env ? <span>{t('settings.mcp.envConfigured')}</span> : null}
        {server.has_headers ? <span>{t('settings.mcp.headersConfigured')}</span> : null}
        {server.last_error ? <span className="ark-settings__hint--error">last_error: {server.last_error}</span> : null}
      </div>

      {shown?.error ? <Notice tone="error" title={t('settings.mcp.testRequestFailed')}>{shown.error}</Notice> : null}
      {shown?.result ? (
        <Notice
          tone={shown.result.ok ? 'warn' : 'error'}
          title={shown.result.ok ? t('settings.mcp.testPassed') : t('settings.mcp.testNotPassed')}
        >
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
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
      return { error: translateNow('settings.mcp.error.mapNotObject', { label }) }
    }
    return { value: Object.fromEntries(Object.entries(value as Record<string, unknown>).map(([key, item]) => [key, String(item)])) }
  } catch {
    return { error: translateNow('settings.mcp.error.mapNotJson', { label }) }
  }
}

export function McpSection(): JSX.Element {
  const { t } = useI18n()
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
      setFormError(t('settings.mcp.error.idRequired'))
      return
    }
    const isStdio = draft.transport === 'stdio'
    const command = draft.command.trim()
    const url = draft.url.trim()
    if (isStdio && !command) {
      setFormError(t('settings.mcp.error.stdioNeedsCommand'))
      return
    }
    if (!isStdio && !url) {
      setFormError(t('settings.mcp.error.httpNeedsUrl'))
      return
    }
    const env = parseMap(draft.envText, 'env')
    const headers = parseMap(draft.headersText, 'headers')
    if (env.error || headers.error) {
      setFormError(env.error ?? headers.error ?? t('settings.mcp.error.advancedInvalid'))
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
      toast.push('success', t('settings.mcp.saved', { id }))
      setDraft(null)
      setEditing(false)
      return
    }
    const message = describeOutcome(outcome)
    setFormError(message)
    toast.push('error', t('settings.mcp.saveFailed'), message)
  }

  const remove = async (serverId: string): Promise<void> => {
    const outcome = await api.remove(serverId)
    if (outcome.ok) toast.push('success', t('settings.mcp.deleted', { id: serverId }))
    else toast.push('error', t('settings.mcp.deleteFailed'), describeOutcome(outcome))
  }

  const test = async (serverId: string): Promise<void> => {
    setTestState({ serverId })
    const outcome = await api.test(serverId)
    if (outcome.ok) {
      setTestState({ serverId, result: outcome.value })
      if (outcome.value.ok) {
        toast.push(
          'success',
          t('settings.mcp.testPassedToast'),
          t('settings.mcp.testSummary', { count: outcome.value.tools.length, latency: outcome.value.latency_ms })
        )
      } else toast.push('error', t('settings.mcp.testFailedToast'), outcome.value.message)
      void api.refresh()
      return
    }
    const message = describeOutcome(outcome)
    setTestState({ serverId, error: message })
    toast.push('error', t('settings.mcp.testRequestFailedToast'), message)
  }

  const body = ((): JSX.Element => {
    if ((api.status === 'loading' || api.status === 'idle') && api.servers.length === 0) return <Skeleton rows={2} />
    if (api.status === 'error') {
      return <LoadError message={api.error ?? t('settings.mcp.loadFailed')} onRetry={() => void api.refresh()} />
    }
    if (api.servers.length === 0) {
      return <EmptyState title={t('settings.mcp.emptyTitle')} hint={t('settings.mcp.emptyHint')} />
    }
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
      title={t('settings.mcp.title')}
      desc={t('settings.mcp.desc')}
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
            {t('settings.mcp.add')}
          </Button>
        )
      }
    >
      {body}

      {draft ? (
        <div className="ark-card ark-card--editing">
          <div className="ark-card__meta">
            <strong>
              {editing ? t('settings.mcp.editTitle', { id: draft.id }) : t('settings.mcp.createTitle')}
            </strong>
          </div>
          <div className="ark-settings__grid">
            <TextField id="mcp-id" label="id" value={draft.id} mono disabled={editing || api.busy !== null} placeholder="filesystem" onChange={(value) => setDraft({ ...draft, id: value })} />
            <SelectField<McpTransportKind>
              id="mcp-transport"
              label="transport"
              value={draft.transport}
              options={['stdio', 'streamable_http']}
              labels={{
                stdio: t(TRANSPORT_LABEL_KEYS.stdio),
                sse: t(TRANSPORT_LABEL_KEYS.sse),
                streamable_http: t(TRANSPORT_LABEL_KEYS.streamable_http)
              }}
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
                <Field label={t('settings.mcp.argsLabel')} htmlFor="mcp-args" hint={t('settings.mcp.argsHint')}>
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
            <CheckField
              label={t('settings.mcp.enabledLabel')}
              checked={draft.enabled}
              disabled={api.busy !== null}
              onChange={(value) => setDraft({ ...draft, enabled: value })}
            />
          </div>

          <div className="ark-settings__grid">
            <Field label={t('settings.mcp.allowToolsLabel')} htmlFor="mcp-allow-tools">
              <textarea id="mcp-allow-tools" className="ark-field__area" value={draft.allowTools} disabled={api.busy !== null} spellCheck={false} onChange={(event) => setDraft({ ...draft, allowTools: event.target.value })} />
            </Field>
            <Field
              label={t('settings.mcp.envLabel')}
              htmlFor="mcp-env"
              hint={editing ? t('settings.mcp.envHintEditing') : t('settings.mcp.envHintCreating')}
            >
              <textarea id="mcp-env" className="ark-field__area" value={draft.envText} disabled={api.busy !== null} spellCheck={false} placeholder={'{\n  "TOKEN": "…"\n}'} onChange={(event) => setDraft({ ...draft, envText: event.target.value })} />
            </Field>
            <Field
              label={t('settings.mcp.headersLabel')}
              htmlFor="mcp-headers"
              hint={editing ? t('settings.mcp.headersHintEditing') : t('settings.mcp.headersHintCreating')}
            >
              <textarea id="mcp-headers" className="ark-field__area" value={draft.headersText} disabled={api.busy !== null} spellCheck={false} placeholder={'{\n  "Authorization": "Bearer …"\n}'} onChange={(event) => setDraft({ ...draft, headersText: event.target.value })} />
            </Field>
          </div>

          {formError ? <Notice tone="error" title={t('settings.mcp.cannotSave')}>{formError}</Notice> : null}

          <div className="ark-settings__actions">
            <Button variant="primary" size="sm" disabled={api.busy !== null} onClick={() => void submit()}>
              {api.busy ? t('common.saving') : t('common.save')}
            </Button>
            <Button variant="ghost" size="sm" disabled={api.busy !== null} onClick={() => { setDraft(null); setEditing(false) }}>
              {t('common.cancel')}
            </Button>
          </div>
        </div>
      ) : null}
    </SettingsSection>
  )
}
