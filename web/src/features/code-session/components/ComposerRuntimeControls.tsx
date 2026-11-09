/**
 * Composer toolbar pills, laid out like the smart-ark Code composer:
 * left — the amber autonomy pill; right — workspace, context ring and the
 * combined provider:model/reasoning pill. Each picker re-reads the stored
 * overrides when it opens and writes through `PATCH /v1/threads/{id}` on
 * every change, so the value applies to the thread's next turn. Fields the
 * runtime cannot clear (provider, autonomy) are never offered as "unset":
 * the PATCH merge keeps the stored value when the patch omits a field, so
 * clearing would be a silent no-op. Empty `reasoning_effort` is the
 * documented "follow default" value.
 */
import { useEffect, useRef, useState } from 'react'
import { Button, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import type { MessageKey } from '../../../shared/i18n'
import { translateNow } from '../../../shared/i18n/current'
import { describeError } from '../../../shared/api/error-describe'
import type { ThreadRuntimeSettings } from '../../../shared/contracts'
import { compactThread, getThread, updateThread } from '../api/threads-api'
import { pickWorkspaceFolder } from '../api/workspace-api'
import { getRuntimeConfig } from '../../settings/api/settings-api'
import { useProviders } from '../../settings/hooks/use-providers'
import { useCodeSessionStore } from '../state/store'
import { canSaveRuntime, isRuntimeFormStale, type RuntimeOwner } from './session-runtime-guard'
import { useContextLimit } from '../hooks/use-context-limit'
import { formatTokens } from '../../../shared/format'
import { ModelProviderMenu, toModelProviderGroups } from '../../../shared/model-provider-menu'

/** Shown until the global default resolves; never persisted on its own. */
const FALLBACK_AUTONOMY = 'workspace_write'

type AutonomyValue = 'strict' | 'workspace_write' | 'unattended'

const REASONING_LABEL: Record<'off' | 'low' | 'medium' | 'high', MessageKey> = {
  off: 'session.runtime.reasoning.off',
  low: 'session.runtime.reasoning.low',
  medium: 'session.runtime.reasoning.medium',
  high: 'session.runtime.reasoning.high'
}

/** Stored values outside the accepted vocabulary fall back to "medium". */
function reasoningLabelKey(value: string): MessageKey {
  return (REASONING_LABEL as Record<string, MessageKey | undefined>)[value] ?? REASONING_LABEL.medium
}

const AUTONOMY_LABEL: Record<AutonomyValue, MessageKey> = {
  strict: 'canvas.composer.runtime.autonomy.strict',
  workspace_write: 'canvas.composer.runtime.autonomy.workspace',
  unattended: 'canvas.composer.runtime.autonomy.full'
}

const AUTONOMY_DESC: Record<AutonomyValue, MessageKey> = {
  strict: 'session.runtime.strict',
  workspace_write: 'session.runtime.workspaceWrite',
  unattended: 'session.runtime.unattended'
}

const AUTONOMY_ORDER: AutonomyValue[] = ['strict', 'workspace_write', 'unattended']

/** Last path segment, so the pill shows 「ark」 instead of the full path. */
function basename(path: string): string {
  const parts = path.trim().split(/[\\/]/).filter(Boolean)
  return parts[parts.length - 1] ?? ''
}

function CaretGlyph(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="10" height="10" aria-hidden="true" focusable="false" className="ark-composer__pill-caret">
      <path d="m6 9.5 6 6 6-6" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  )
}

function LockGlyph(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="13" height="13" aria-hidden="true" focusable="false">
      <rect x="5.5" y="10.5" width="13" height="9" rx="2.2" fill="none" stroke="currentColor" strokeWidth="1.8" />
      <path d="M8.5 10.5V8a3.5 3.5 0 0 1 7 0v2.5" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
    </svg>
  )
}

function FolderGlyph(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true" focusable="false">
      <path
        d="M3.5 7.2c0-1 .8-1.7 1.7-1.7h4.1l2 2.2h7.5c1 0 1.7.8 1.7 1.7v8.4c0 1-.8 1.7-1.7 1.7H5.2c-1 0-1.7-.8-1.7-1.7V7.2Z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.7"
        strokeLinejoin="round"
      />
    </svg>
  )
}

/** Outside-click + Escape dismissal shared by every toolbar popover. */
function useDismiss(open: boolean, rootRef: React.RefObject<HTMLElement | null>, close: () => void): void {
  useEffect(() => {
    if (!open) return
    const onDown = (event: MouseEvent): void => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) close()
    }
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') close()
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open, rootRef, close])
}

async function fetchRuntime(threadId: string): Promise<ThreadRuntimeSettings> {
  const detail = await getThread(threadId)
  return { ...detail.runtime }
}

/* ---------- left: amber autonomy pill ---------- */

export function AutonomyPicker({ threadId }: { threadId: string | null }): JSX.Element {
  const { t } = useI18n()
  const setCommandError = useCodeSessionStore((s) => s.setCommandError)
  const [runtime, setRuntime] = useState<ThreadRuntimeSettings>({})
  const [defaultAutonomy, setDefaultAutonomy] = useState(FALLBACK_AUTONOMY)
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const rootRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    void getRuntimeConfig()
      .then((config) => setDefaultAutonomy(config.default_autonomy))
      .catch(() => {
        // Keep the fallback: an unreadable config must not block the toolbar.
      })
  }, [])

  // Opening the pill loads the stored overrides fresh, so a cached copy can
  // never write a stale value back over another editor's change.
  useEffect(() => {
    if (!open || !threadId) return
    void fetchRuntime(threadId)
      .then(setRuntime)
      .catch((error: unknown) => setCommandError(describeError(error, translateNow)))
  }, [open, threadId, setCommandError])

  useDismiss(open, rootRef, () => setOpen(false))

  const apply = async (patch: ThreadRuntimeSettings): Promise<void> => {
    if (!threadId || busy) return
    setBusy(true)
    try {
      await updateThread(threadId, { runtime: patch })
      setRuntime(await fetchRuntime(threadId))
    } catch (error) {
      setCommandError(describeError(error, translateNow))
    } finally {
      setBusy(false)
    }
  }

  const autonomyValue = (runtime.autonomy ?? defaultAutonomy) as AutonomyValue
  const disabled = !threadId || busy

  return (
    <div className="ark-composer__pick" ref={rootRef}>
      <button
        type="button"
        className="ark-composer__pill ark-composer__pill--autonomy"
        disabled={disabled}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={t('canvas.composer.runtime.autonomyAria')}
        onClick={() => setOpen((value) => !value)}
      >
        <LockGlyph />
        <span className="ark-composer__pill-label">{t(AUTONOMY_LABEL[autonomyValue])}</span>
        <CaretGlyph />
      </button>
      {open ? (
        <div className="ark-composer__popover" role="menu" aria-label={t('canvas.composer.runtime.autonomyAria')}>
          {AUTONOMY_ORDER.map((value) => (
            <button
              key={value}
              type="button"
              className="ark-composer__option"
              role="menuitemradio"
              aria-selected={autonomyValue === value}
              onClick={() => {
                setOpen(false)
                void apply({ autonomy: value })
              }}
            >
              {t(AUTONOMY_LABEL[value])}
              <span className="ark-composer__option-desc">{t(AUTONOMY_DESC[value])}</span>
            </button>
          ))}
        </div>
      ) : null}
    </div>
  )
}

/* ---------- right: workspace pill ---------- */

export function WorkspacePicker({ threadId }: { threadId: string | null }): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const setCommandError = useCodeSessionStore((s) => s.setCommandError)
  const [runtime, setRuntime] = useState<ThreadRuntimeSettings>({})
  const [draft, setDraft] = useState('')
  const [open, setOpen] = useState(false)
  const [saving, setSaving] = useState(false)
  const [picking, setPicking] = useState(false)
  const [loadedFor, setLoadedFor] = useState<RuntimeOwner>(null)
  const paintedThreadId = useRef(threadId)
  const rootRef = useRef<HTMLDivElement | null>(null)

  // Switching threads must blank the form in the same commit, or the pill
  // would offer to save A's workspace onto B.
  if (isRuntimeFormStale(paintedThreadId.current ?? '', threadId ?? '')) {
    paintedThreadId.current = threadId
    setRuntime({})
    setDraft('')
    setLoadedFor(null)
    setSaving(false)
  }

  useEffect(() => {
    if (!threadId) return
    let active = true
    setLoadedFor(null)
    void fetchRuntime(threadId)
      .then((value) => {
        if (!active) return
        setRuntime(value)
        setDraft(value.workspace ?? '')
        setLoadedFor(threadId)
      })
      .catch(() => {
        if (!active) return
        // A failed load leaves `loadedFor` null, so the save cannot write
        // another thread's stored overrides.
        setLoadedFor(null)
      })
    return () => {
      active = false
    }
  }, [threadId])

  useDismiss(open, rootRef, () => setOpen(false))

  const save = async (): Promise<void> => {
    if (!threadId) return
    // Drop the write instead of applying displayed values to another thread.
    if (!canSaveRuntime(loadedFor, threadId)) return
    setSaving(true)
    try {
      // Only the workspace override travels; omitted fields keep their values.
      await updateThread(threadId, { runtime: { workspace: draft || undefined } })
      const next = await fetchRuntime(threadId)
      setRuntime(next)
      setDraft(next.workspace ?? '')
      toast.push('success', t('session.runtime.title'))
    } catch (error) {
      setCommandError(describeError(error, translateNow))
      toast.push('error', t('session.runtime.title'), error instanceof Error ? error.message : String(error))
    } finally {
      setSaving(false)
    }
  }

  // Opens the host's native folder dialog (POST /v1/workspace/pick); the
  // picked path lands in the input so the explicit save stays the only
  // write path. `path` is undefined when the user cancelled the dialog.
  const browse = async (): Promise<void> => {
    if (picking) return
    setPicking(true)
    try {
      const picked = await pickWorkspaceFolder()
      if (picked.path) setDraft(picked.path)
    } catch (error) {
      toast.push('error', t('session.runtime.workspace'), error instanceof Error ? error.message : String(error))
    } finally {
      setPicking(false)
    }
  }

  const label = basename(runtime.workspace ?? '')

  return (
    <div className="ark-composer__pick ark-composer__pick--end" ref={rootRef}>
      <button
        type="button"
        className="ark-composer__pill"
        disabled={!threadId}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={t('canvas.composer.workspaceAria')}
        title={runtime.workspace || t('common.notSet')}
        onClick={() => setOpen((value) => !value)}
      >
        <FolderGlyph />
        <span className="ark-composer__pill-label">{label || t('common.notSet')}</span>
        <CaretGlyph />
      </button>
      {open ? (
        <div className="ark-composer__popover" role="group" aria-label={t('canvas.composer.workspaceAria')}>
          <label className="ark-composer__field">
            <span>{t('session.runtime.workspace')}</span>
            <input
              value={draft}
              placeholder={t('workbench.changes.workspacePlaceholder')}
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') {
                  event.preventDefault()
                  void save()
                }
              }}
            />
          </label>
          <div className="ark-composer__popover-actions">
            <Button
              size="sm"
              disabled={picking}
              onClick={() => void browse()}
            >
              {picking ? t('common.working') : t('session.runtime.browse')}
            </Button>
            <Button
              size="sm"
              variant="primary"
              disabled={saving || threadId === null || !canSaveRuntime(loadedFor, threadId)}
              onClick={() => void save()}
            >
              {saving ? t('common.saving') : t('session.runtime.save')}
            </Button>
          </div>
        </div>
      ) : null}
    </div>
  )
}

/* ---------- right: context capacity ring ---------- */

const RING_SIZE = 26
const RING_RADIUS = 10.5
const RING_CIRCUMFERENCE = 2 * Math.PI * RING_RADIUS
const RING_STROKE = 2.5
/** Past this fraction of the limit the ring turns the warning tone. */
const RING_WARN_RATIO = 0.8

export function ContextRing({ threadId }: { threadId: string | null }): JSX.Element | null {
  const { t } = useI18n()
  const toast = useToast()
  const usage = useCodeSessionStore((s) => s.usage)
  const contextLimit = useContextLimit()
  const [open, setOpen] = useState(false)
  const [compacting, setCompacting] = useState(false)
  const rootRef = useRef<HTMLDivElement | null>(null)

  useDismiss(open, rootRef, () => setOpen(false))

  if (!usage || !contextLimit || contextLimit <= 0 || usage.lastPromptTokens == null) return null

  const ratio = Math.min(1, Math.max(0, usage.lastPromptTokens / contextLimit))
  const percent = Math.round(ratio * 100)
  const warn = ratio >= RING_WARN_RATIO

  const compact = async (): Promise<void> => {
    if (!threadId || compacting) return
    setCompacting(true)
    try {
      await compactThread(threadId)
      toast.push('success', t('session.context.compacted'))
    } catch (error) {
      toast.push('error', t('session.context.compact'), error instanceof Error ? error.message : String(error))
    } finally {
      setCompacting(false)
    }
  }

  return (
    <div className="ark-composer__pick ark-composer__pick--end" ref={rootRef}>
      <button
        type="button"
        className={`ark-composer__ring${warn ? ' is-warn' : ''}`}
        aria-label={t('canvas.usage.contextTitle')}
        aria-expanded={open}
        title={t('canvas.usage.contextTitle')}
        onClick={() => setOpen((value) => !value)}
      >
        <svg viewBox={`0 0 ${RING_SIZE} ${RING_SIZE}`} width={RING_SIZE} height={RING_SIZE} aria-hidden="true" focusable="false">
          <circle cx={RING_SIZE / 2} cy={RING_SIZE / 2} r={RING_RADIUS} fill="none" stroke="var(--ark-bg-inset)" strokeWidth={RING_STROKE} />
          <circle
            cx={RING_SIZE / 2}
            cy={RING_SIZE / 2}
            r={RING_RADIUS}
            fill="none"
            stroke={warn ? 'var(--ark-warn)' : 'var(--ark-accent)'}
            strokeWidth={RING_STROKE}
            strokeLinecap="round"
            strokeDasharray={RING_CIRCUMFERENCE}
            strokeDashoffset={RING_CIRCUMFERENCE * (1 - ratio)}
            transform={`rotate(-90 ${RING_SIZE / 2} ${RING_SIZE / 2})`}
          />
        </svg>
        <span className="ark-composer__ring-num">{percent}</span>
      </button>
      {open ? (
        <div className="ark-composer__popover" role="group" aria-label={t('canvas.usage.contextTitle')}>
          <p className="ark-composer__hint">
            {t('canvas.usage.context', {
              used: formatTokens(usage.lastPromptTokens ?? 0),
              limit: formatTokens(contextLimit),
              percent
            })}
          </p>
          <Button size="sm" disabled={compacting || !threadId} onClick={() => void compact()}>
            {compacting ? t('common.refreshing') : t('session.context.compact')}
          </Button>
        </div>
      ) : null}
    </div>
  )
}

/* ---------- right: combined provider:model / reasoning pill ---------- */

/**
 * ONE pill opens the shared two-level provider -> model menu: the left column
 * lists providers, the right column only that provider's models, and picking a
 * model pins both ids together. Reasoning effort stays in the menu footer.
 * Re-reads stored overrides on open and writes through PATCH /v1/threads/{id}.
 */
export function ModelPicker({ threadId }: { threadId: string | null }): JSX.Element {
  const { t } = useI18n()
  const providers = useProviders()
  const setCommandError = useCodeSessionStore((s) => s.setCommandError)
  const [runtime, setRuntime] = useState<ThreadRuntimeSettings>({})
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (!open || !threadId) return
    void fetchRuntime(threadId)
      .then(setRuntime)
      .catch((error: unknown) => setCommandError(describeError(error, translateNow)))
  }, [open, threadId, setCommandError])

  const apply = async (patch: ThreadRuntimeSettings): Promise<void> => {
    if (!threadId || busy) return
    setBusy(true)
    try {
      await updateThread(threadId, { runtime: patch })
      setRuntime(await fetchRuntime(threadId))
    } catch (error) {
      setCommandError(describeError(error, translateNow))
    } finally {
      setBusy(false)
    }
  }

  const groups = toModelProviderGroups(providers.providers)
  const defaultProvider = providers.providers.find((provider) => provider.is_default) ?? null
  // The provider a turn would actually use: the thread override, else the
  // configured default. The vision warning keys off it.
  const resolvedProvider =
    providers.providers.find((provider) => provider.id === (runtime.provider_id ?? '')) ??
    defaultProvider ??
    null
  const fallback = defaultProvider
    ? { providerId: defaultProvider.id, model: defaultProvider.model }
    : undefined
  const visionUnsupported = resolvedProvider !== null && resolvedProvider.supports_vision !== true

  const modelValue = runtime.model || resolvedProvider?.model || ''
  const providerLabel = resolvedProvider ? resolvedProvider.label ?? resolvedProvider.id : ''
  const reasoningValue = runtime.reasoning_effort ?? ''
  const reasoningLabel = reasoningValue ? t(reasoningLabelKey(reasoningValue)) : t('canvas.composer.runtime.default')
  const pillLabel = [providerLabel, modelValue].filter(Boolean).join(':') || t('canvas.composer.runtime.default')
  const disabled = !threadId || busy

  return (
    <ModelProviderMenu
      className="ark-mpm--inline"
      menuClassName="ark-mpm__menu--above-end"
      groups={groups}
      value={{ providerId: runtime.provider_id ?? '', model: runtime.model ?? '' }}
      fallback={fallback}
      open={open}
      onOpenChange={setOpen}
      disabled={disabled}
      ariaLabel={t('canvas.composer.runtime.modelAria')}
      title={pillLabel}
      triggerClassName="ark-composer__pill ark-composer__pill--model"
      trigger={
        <>
          <span className="ark-composer__pill-label">{pillLabel}</span>
          <span className="ark-composer__pill-effort">{reasoningLabel}</span>
          <CaretGlyph />
        </>
      }
      header={visionUnsupported ? <p className="ark-mpm__hint">{t('session.runtime.visionUnsupported')}</p> : null}
      onSelect={(selection) => {
        // merge_runtime keeps omitted fields, so echoing the stored runtime
        // applies the new pair without clearing autonomy/workspace overrides.
        void apply({ ...runtime, provider_id: selection.providerId, model: selection.model })
      }}
      footer={(close) => (
        <div className="ark-composer__reasoning" role="group" aria-label={t('canvas.composer.runtime.reasoningAria')}>
          <button
            type="button"
            className="ark-composer__option"
            role="menuitemradio"
            aria-selected={reasoningValue === ''}
            onClick={() => {
              close()
              // Empty string is the documented "unset": the runtime drops it
              // and falls back to the provider's configured reasoning effort.
              void apply({ reasoning_effort: '' })
            }}
          >
            {t('canvas.composer.runtime.default')}
            <span className="ark-composer__option-desc">{t('canvas.composer.runtime.followDefault')}</span>
          </button>
          {(Object.keys(REASONING_LABEL) as Array<keyof typeof REASONING_LABEL>).map((value) => (
            <button
              key={value}
              type="button"
              className="ark-composer__option"
              role="menuitemradio"
              aria-selected={reasoningValue === value}
              onClick={() => {
                close()
                void apply({ reasoning_effort: value })
              }}
            >
              {t(REASONING_LABEL[value])}
            </button>
          ))}
        </div>
      )}
    />
  )
}
