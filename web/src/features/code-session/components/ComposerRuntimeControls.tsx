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
import { useEffect, useMemo, useRef, useState } from 'react'
import { Button, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import type { MessageKey } from '../../../shared/i18n'
import { translateNow } from '../../../shared/i18n/current'
import { describeError } from '../../../shared/api/error-describe'
import type { SkillSummary, ThreadRuntimeSettings, WorkspaceView } from '../../../shared/contracts'
import { compactThread, getThread, updateThread } from '../api/threads-api'
import { pickWorkspaceFolder } from '../api/workspace-api'
import { getWorkspaceView, validateWorkspace } from '../../settings/api/workspace-api'
import { listThreadSkills } from '../api/skills-api'
import { getRuntimeConfig } from '../../settings/api/settings-api'
import { useProviders } from '../../settings/hooks/use-providers'
import { useCodeSessionStore } from '../state/store'
import { canSaveRuntime, isRuntimeFormStale, type RuntimeOwner } from './session-runtime-guard'
import { useContextLimit } from '../hooks/use-context-limit'
import { orderedItems } from '../state/selectors'
import { CONTEXT_CATEGORY_ORDER, contextStructure, type ContextCategoryKey, type ContextStructure } from './context-structure'
import { workspaceBasename, workspaceOptions } from './workspace-options'
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

/* ---------- right: workspace project picker ---------- */

/**
 * One pill opens a list of the host's active + recent projects. Picking a row
 * pins the path as this thread's session-level override (`PATCH
 * /v1/threads/{id}`), so the next turn of THIS thread resolves it (runner
 * leases are per-turn); "add" opens the native folder chooser and validates
 * the path through the runtime first. The host-global root is not touched:
 * other sessions must stay free to run turns against the old default while
 * this session re-points. No path is typed by hand.
 */
export function WorkspacePicker({ threadId }: { threadId: string | null }): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const setCommandError = useCodeSessionStore((s) => s.setCommandError)
  const activeTurnId = useCodeSessionStore((s) => s.activeTurnId)
  const [runtime, setRuntime] = useState<ThreadRuntimeSettings>({})
  const [view, setView] = useState<WorkspaceView | null>(null)
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [busy, setBusy] = useState(false)
  const [loadedFor, setLoadedFor] = useState<RuntimeOwner>(null)
  const paintedThreadId = useRef(threadId)
  const rootRef = useRef<HTMLDivElement | null>(null)

  if (isRuntimeFormStale(paintedThreadId.current ?? '', threadId ?? '')) {
    paintedThreadId.current = threadId
    setRuntime({})
    setLoadedFor(null)
  }

  useEffect(() => {
    if (!threadId) return
    let active = true
    setLoadedFor(null)
    void fetchRuntime(threadId)
      .then((value) => {
        if (!active) return
        setRuntime(value)
        setLoadedFor(threadId)
      })
      .catch(() => { if (active) setLoadedFor(null) })
    return () => { active = false }
  }, [threadId])

  useEffect(() => {
    // The pill label falls back to the host's active workspace when this
    // thread has no override (that is exactly what a turn resolves), so load
    // the view as soon as a thread is selected — not only when the menu opens.
    let active = true
    void getWorkspaceView()
      .then((value) => { if (active) setView(value) })
      .catch(() => { /* the label then falls back to （未设置） */ })
    return () => { active = false }
  }, [threadId])

  useEffect(() => {
    if (!open) return
    let active = true
    void getWorkspaceView().then((value) => {
      if (active) setView(value)
    }).catch((error: unknown) => {
      if (active) toast.push('error', t('canvas.workspace.switchFailed'), describeError(error, translateNow))
    })
    return () => { active = false }
  }, [open, toast, t])

  useDismiss(open, rootRef, () => setOpen(false))

  const { current, options } = useMemo(
    () => workspaceOptions({ override: runtime.workspace, active: view?.active, recent: view?.recent ?? [] }),
    [runtime.workspace, view]
  )
  const needle = query.trim().toLowerCase()
  const filtered = needle
    ? options.filter((option) => option.label.toLowerCase().includes(needle) || option.path.toLowerCase().includes(needle))
    : options

  const canSwitch = threadId !== null && canSaveRuntime(loadedFor, threadId) && activeTurnId === null

  const commitWorkspace = async (
    path: string,
    messageKey: 'canvas.workspace.selected' | 'canvas.workspace.added'
  ): Promise<void> => {
    if (!threadId || !canSwitch) return
    // Session-level semantics (matching mainstream coding agents): the pick
    // writes this thread's override and applies to this thread's next turn.
    // Only the model currently streaming in THIS thread blocks the change —
    // the global switch (with its process-wide busy 409) stays on the
    // settings/changes panels and is deliberately not part of this flow.
    await updateThread(threadId, { runtime: { workspace: path } })
    // Reflect the switch in the rail right away so the project grouping moves
    // with it; the next list refresh stays authoritative.
    useCodeSessionStore.getState().setThreadWorkspaceRow(threadId, path)
    const [nextView, nextRuntime] = await Promise.all([getWorkspaceView(), fetchRuntime(threadId)])
    setView(nextView)
    setRuntime(nextRuntime)
    setLoadedFor(threadId)
    toast.push('success', t(messageKey))
    setOpen(false)
    setQuery('')
  }

  const selectWorkspace = async (path: string): Promise<void> => {
    if (busy || !canSwitch) return
    setBusy(true)
    try {
      await commitWorkspace(path, 'canvas.workspace.selected')
    } catch (error) {
      const detail = describeError(error, translateNow)
      setCommandError(detail)
      toast.push('error', t('canvas.workspace.switchFailed'), detail)
    } finally {
      setBusy(false)
    }
  }

  const addProject = async (): Promise<void> => {
    if (busy || !canSwitch) return
    setBusy(true)
    try {
      const picked = await pickWorkspaceFolder()
      if (!picked.path) return
      const validated = await validateWorkspace(picked.path)
      await commitWorkspace(validated.path, 'canvas.workspace.added')
    } catch (error) {
      const detail = describeError(error, translateNow)
      setCommandError(detail)
      toast.push('error', t('canvas.workspace.addFailed'), detail)
    } finally {
      setBusy(false)
    }
  }

  const busyHint = activeTurnId !== null ? t('canvas.workspace.busy') : ''
  const label = workspaceBasename(current) || t('common.notSet')

  return (
    <div className="ark-composer__pick ark-composer__pick--end" ref={rootRef}>
      <button
        type="button"
        className="ark-composer__pill"
        disabled={!threadId}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={t('canvas.composer.workspaceAria')}
        title={current || t('common.notSet')}
        onClick={() => setOpen((value) => !value)}
      >
        <FolderGlyph />
        <span className="ark-composer__pill-label">{label}</span>
        <CaretGlyph />
      </button>
      {open ? (
        <div className="ark-composer__popover ark-workspace-menu" role="group" aria-label={t('canvas.composer.workspaceAria')}>
          <label className="ark-composer__field">
            <span>{t('canvas.workspace.projects')}</span>
            <input
              type="search"
              value={query}
              placeholder={t('canvas.workspace.search')}
              onChange={(event) => setQuery(event.target.value)}
            />
          </label>
          <div className="ark-workspace-menu__list">
            {filtered.map((option) => {
              const isCurrent = option.path === current
              return (
                <button
                  key={option.path}
                  type="button"
                  className={`ark-workspace-menu__option${isCurrent ? ' is-current' : ''}`}
                  disabled={busy || !canSwitch}
                  title={busyHint || option.path}
                  onClick={() => void selectWorkspace(option.path)}
                >
                  <span className="ark-workspace-menu__label">{option.label}</span>
                  {option.parent ? <span className="ark-workspace-menu__context">{option.parent}</span> : null}
                  {isCurrent ? <span className="ark-workspace-menu__check" aria-label={t('canvas.workspace.current')}>✓</span> : null}
                </button>
              )
            })}
            {filtered.length === 0 ? (
              <p className="ark-composer__hint">{options.length === 0 ? t('canvas.workspace.empty') : t('canvas.workspace.noMatch')}</p>
            ) : null}
          </div>
          <Button size="sm" disabled={busy || !canSwitch} title={busyHint} onClick={() => void addProject()}>
            {busy ? t('common.working') : t('canvas.workspace.addProject')}
          </Button>
        </div>
      ) : null}
    </div>
  )
}

/* ---------- right: context structure + manual compaction ---------- */

const RING_SIZE = 26
const RING_RADIUS = 10.5
const RING_CIRCUMFERENCE = 2 * Math.PI * RING_RADIUS
const RING_STROKE = 2.5

/** Stable empty view for the closed-popover memo. */
const CLOSED_ITEM_VIEW: { itemOrder: string[]; itemsById: Record<string, never> } = {
  itemOrder: [],
  itemsById: {}
}

const CATEGORY_LABEL: Record<ContextCategoryKey, MessageKey> = {
  tools: 'canvas.context.tools',
  system: 'canvas.context.system',
  skills: 'canvas.context.skills',
  messages: 'canvas.context.messages',
  reasoning: 'canvas.context.reasoning',
  checkpoint: 'canvas.context.checkpoint',
  other: 'canvas.context.other'
}

/**
 * The ring is the compact entry point; its popover shows the *shape* of the
 * durable conversation (render rows + loaded skills) beside the authoritative
 * prompt-token total. Token arithmetic stays in `ark-agent::TokenEstimator` —
 * this panel deliberately does not estimate tokens from text.
 */
export function ContextRing({ threadId }: { threadId: string | null }): JSX.Element | null {
  const { t } = useI18n()
  const toast = useToast()
  const usage = useCodeSessionStore((s) => s.usage)
  const itemOrder = useCodeSessionStore((s) => s.itemOrder)
  const itemsById = useCodeSessionStore((s) => s.itemsById)
  const reading = useContextLimit()
  const [open, setOpen] = useState(false)
  const [skills, setSkills] = useState<SkillSummary[]>([])
  const [compacting, setCompacting] = useState(false)
  const rootRef = useRef<HTMLDivElement | null>(null)

  useDismiss(open, rootRef, () => setOpen(false))

  useEffect(() => {
    if (!open || !threadId) return
    let active = true
    void listThreadSkills(threadId)
      .then((view) => { if (active) setSkills([...view.base_skills, ...view.extra_skills]) })
      .catch(() => { if (active) setSkills([]) })
    return () => { active = false }
  }, [open, threadId])

  // While the popover is closed, feed a stable empty view into the memo: token
  // deltas must not rescan the whole conversation for a hidden panel.
  const snapshot = open ? { itemOrder, itemsById } : CLOSED_ITEM_VIEW
  const structure: ContextStructure = useMemo(
    () => contextStructure(orderedItems(snapshot), skills),
    [snapshot, skills]
  )

  if (!threadId) return null

  // The ring fills against the model's real context window; the compaction
  // threshold rides inside it as the warning line (`threshold / window`).
  const limit = reading.window && reading.window > 0 ? reading.window : null
  const used = usage?.lastPromptTokens ?? null
  const ratio = limit && used != null ? Math.min(1, Math.max(0, used / limit)) : null
  const percent = ratio == null ? null : Math.round(ratio * 100)
  const thresholdRatio =
    limit && reading.threshold != null ? Math.min(1, Math.max(0, reading.threshold / limit)) : null
  const warn = ratio != null && thresholdRatio != null && ratio >= thresholdRatio
  const free = limit && used != null ? Math.max(0, limit - used) : null
  const freeRatio = limit && free != null ? free / limit : null
  const compact = async (): Promise<void> => {
    if (!threadId || compacting) return
    setCompacting(true)
    try {
      await compactThread(threadId)
      toast.push('success', t('session.context.compacted'))
    } catch (error) {
      toast.push('error', t('session.context.compact'), describeError(error, translateNow))
    } finally {
      setCompacting(false)
    }
  }

  const statusKey = ratio != null && thresholdRatio != null && ratio >= 1
    ? 'canvas.context.overLimit'
    : ratio != null && thresholdRatio != null && ratio >= thresholdRatio
      ? 'canvas.context.nearLimit'
      : 'canvas.context.hasRoom'

  return (
    <div className="ark-composer__pick ark-composer__pick--end" ref={rootRef}>
      <button
        type="button"
        className={`ark-composer__ring${warn ? ' is-warn' : ''}`}
        aria-label={t('canvas.context.title')}
        aria-expanded={open}
        title={t('canvas.context.title')}
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
            strokeDashoffset={RING_CIRCUMFERENCE * (1 - (ratio ?? 0))}
            transform={`rotate(-90 ${RING_SIZE / 2} ${RING_SIZE / 2})`}
          />
        </svg>
        <span className="ark-composer__ring-num">{percent == null ? '·' : percent}</span>
      </button>
      {open ? (
        <div className="ark-composer__popover ark-context-popover" role="group" aria-label={t('canvas.context.title')}>
          <div className="ark-context-popover__head">
            <strong>{t('canvas.context.title')}</strong>
            <span>
              {limit && used != null
                ? t('canvas.usage.context', { used: formatTokens(used), limit: formatTokens(limit), percent: percent ?? 0 })
                : t('canvas.context.noLimit')}
            </span>
          </div>
          <div className="ark-context-popover__bar" role="img" aria-label={t('canvas.context.title')}>
            {structure.segments.filter((segment) => segment.ratio > 0).map((segment) => (
              <span
                key={segment.key}
                className={`ark-context-popover__seg ark-context-popover__seg--${segment.key}`}
                style={{ width: `${segment.ratio * 100}%` }}
              />
            ))}
            {thresholdRatio != null ? (
              <span
                className="ark-context-popover__mark"
                style={{ left: `${thresholdRatio * 100}%` }}
                title={t('canvas.context.threshold', { tokens: formatTokens(reading.threshold ?? 0) })}
              />
            ) : null}
          </div>
          <p className={`ark-context-popover__status${warn ? ' is-warn' : ''}`}>{t(statusKey)}</p>
          {limit ? (
            <p className="ark-context-popover__note">
              {t('canvas.context.window', { tokens: formatTokens(limit) })}
              {reading.threshold != null
                ? t('canvas.context.thresholdNote', { tokens: formatTokens(reading.threshold) })
                : ''}
            </p>
          ) : null}
          <div className="ark-context-popover__rows">
            {CONTEXT_CATEGORY_ORDER.map((key) => {
              const segment = structure.segments.find((entry) => entry.key === key)
              if (!segment) return null
              return (
                <div className="ark-context-popover__row" key={key}>
                  <span className={`ark-context-popover__swatch ark-context-popover__seg--${key}`} />
                  <span className="ark-context-popover__label">{t(CATEGORY_LABEL[key])}</span>
                  <span className="ark-context-popover__value">
                    {t('canvas.context.segment', {
                      items: segment.items,
                      chars: formatTokens(segment.chars),
                      percent: Math.round(segment.ratio * 100)
                    })}
                  </span>
                </div>
              )
            })}
            {free != null && freeRatio != null ? (
              <div className="ark-context-popover__row is-free">
                <span className="ark-context-popover__swatch" />
                <span className="ark-context-popover__label">{t('canvas.context.free')}</span>
                <span className="ark-context-popover__value">
                  {t('canvas.context.freeSegment', { chars: formatTokens(free), percent: Math.round(freeRatio * 100) })}
                </span>
              </div>
            ) : null}
          </div>
          <p className="ark-context-popover__note">{t('canvas.context.estimateHint')}</p>
          <Button size="sm" variant="primary" block disabled={compacting || !threadId} onClick={() => void compact()}>
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
      variant="flyout"
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
