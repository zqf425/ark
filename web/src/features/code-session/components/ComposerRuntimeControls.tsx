/**
 * Quick runtime overrides in the composer toolbar: model/provider, permission
 * mode and reasoning effort. Each picker re-reads the stored overrides when it
 * opens and writes through `PATCH /v1/threads/{id}` on every change, so the
 * value applies to the thread's next turn. Fields the runtime cannot clear
 * (provider, autonomy) are never offered as "unset": the PATCH merge keeps the
 * stored value when the patch omits a field, so clearing would be a silent
 * no-op. Empty `reasoning_effort` is the documented "follow default" value.
 */
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { Button } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import type { MessageKey } from '../../../shared/i18n'
import { translateNow } from '../../../shared/i18n/current'
import { describeError } from '../../../shared/api/error-describe'
import type { ThreadRuntimeSettings } from '../../../shared/contracts'
import { getThread, updateThread } from '../api/threads-api'
import { getRuntimeConfig } from '../../settings/api/settings-api'
import { useProviders } from '../../settings/hooks/use-providers'
import { useCodeSessionStore } from '../state/store'

/** Shown until the global default resolves; never persisted on its own. */
const FALLBACK_AUTONOMY = 'workspace_write'

type PickerId = 'model' | 'autonomy' | 'reasoning'
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

export function ComposerRuntimeControls({ threadId }: { threadId: string | null }): JSX.Element {
  const { t } = useI18n()
  const providers = useProviders()
  const setCommandError = useCodeSessionStore((s) => s.setCommandError)
  const [runtime, setRuntime] = useState<ThreadRuntimeSettings>({})
  const [defaultAutonomy, setDefaultAutonomy] = useState(FALLBACK_AUTONOMY)
  const [open, setOpen] = useState<PickerId | null>(null)
  const [busy, setBusy] = useState(false)
  const rootRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    void getRuntimeConfig()
      .then((config) => setDefaultAutonomy(config.default_autonomy))
      .catch(() => {
        // Keep the fallback: an unreadable config must not block the toolbar.
      })
  }, [])

  // Opening a picker loads the stored overrides fresh: the toolbar coexists
  // with the runtime panel, and a cached copy could write stale values back
  // over another editor's change.
  const loadRuntime = async (): Promise<void> => {
    if (!threadId) return
    try {
      const detail = await getThread(threadId)
      setRuntime({ ...detail.runtime })
    } catch (error) {
      setCommandError(describeError(error, translateNow))
    }
  }

  const toggle = async (picker: PickerId): Promise<void> => {
    const next = open === picker ? null : picker
    setOpen(next)
    if (next !== null) await loadRuntime()
  }

  useEffect(() => {
    if (open === null) return
    const onDown = (event: MouseEvent): void => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) setOpen(null)
    }
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') setOpen(null)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  const apply = async (patch: ThreadRuntimeSettings): Promise<void> => {
    if (!threadId || busy) return
    setBusy(true)
    try {
      await updateThread(threadId, { runtime: patch })
    } catch (error) {
      setCommandError(describeError(error, translateNow))
    } finally {
      // Reload unconditionally: the merge is server-side, so the stored truth
      // (not the optimistic patch) is what the next turn will use.
      await loadRuntime()
      setBusy(false)
    }
  }

  // The provider a turn would actually use: the thread override, else the
  // store's default. Model suggestions and the vision warning key off it.
  const selectedProvider =
    providers.providers.find((provider) => provider.id === (runtime.provider_id ?? '')) ??
    providers.providers.find((provider) => provider.is_default) ??
    null
  const modelOptions = selectedProvider
    ? [selectedProvider.model, ...selectedProvider.models.map((item) => item.id)].filter(
        (id) => id && id !== (runtime.model ?? '')
      )
    : []
  const visionUnsupported = selectedProvider !== null && selectedProvider.supports_vision !== true

  const modelValue = runtime.model || selectedProvider?.model || ''
  const autonomyValue = (runtime.autonomy ?? defaultAutonomy) as AutonomyValue
  const reasoningValue = runtime.reasoning_effort ?? ''
  const disabled = !threadId || busy

  return (
    <div className="ark-composer__pickers" ref={rootRef}>
      <PickerButton
        label={t('canvas.composer.runtime.modelLabel', { value: modelValue || '—' })}
        aria={t('canvas.composer.runtime.modelAria')}
        disabled={disabled}
        expanded={open === 'model'}
        onClick={() => void toggle('model')}
      >
        <Field label={t('session.runtime.provider')}>
          <select
            value={runtime.provider_id ?? ''}
            onChange={(event) => {
              // An unset override falls back to the default provider; an
              // existing one cannot be cleared (the merge keeps it), so the
              // placeholder option is not offered once an override exists.
              const value = event.target.value
              if (value) void apply({ provider_id: value })
            }}
          >
            {runtime.provider_id ? null : <option value="">{t('canvas.composer.runtime.followDefault')}</option>}
            {providers.providers.map((provider) => (
              <option key={provider.id} value={provider.id}>
                {provider.label ?? provider.id}
              </option>
            ))}
          </select>
        </Field>
        <ModelField
          value={runtime.model ?? ''}
          options={modelOptions}
          onCommit={(value) => void apply({ ...runtime, model: value })}
          label={t('session.runtime.model')}
        />
        {visionUnsupported ? <p className="ark-composer__hint">{t('session.runtime.visionUnsupported')}</p> : null}
      </PickerButton>

      <PickerButton
        label={t('canvas.composer.runtime.autonomyLabel', { value: t(AUTONOMY_LABEL[autonomyValue]) })}
        aria={t('canvas.composer.runtime.autonomyAria')}
        disabled={disabled}
        expanded={open === 'autonomy'}
        onClick={() => void toggle('autonomy')}
      >
        {AUTONOMY_ORDER.map((value) => (
          <button
            key={value}
            type="button"
            className="ark-composer__option"
            role="menuitemradio"
            aria-selected={autonomyValue === value}
            onClick={() => {
              setOpen(null)
              void apply({ autonomy: value })
            }}
          >
            {t(AUTONOMY_LABEL[value])}
            <span className="ark-composer__option-desc">{t(AUTONOMY_DESC[value])}</span>
          </button>
        ))}
      </PickerButton>

      <PickerButton
        label={t('canvas.composer.runtime.reasoningLabel', {
          value: reasoningValue ? t(reasoningLabelKey(reasoningValue)) : t('canvas.composer.runtime.default')
        })}
        aria={t('canvas.composer.runtime.reasoningAria')}
        disabled={disabled}
        expanded={open === 'reasoning'}
        onClick={() => void toggle('reasoning')}
      >
        <button
          type="button"
          className="ark-composer__option"
          role="menuitemradio"
          aria-selected={reasoningValue === ''}
          onClick={() => {
            setOpen(null)
            // Empty string is the documented "unset": the runtime drops it and
            // falls back to the provider's configured reasoning effort.
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
              setOpen(null)
              void apply({ reasoning_effort: value })
            }}
          >
            {t(REASONING_LABEL[value])}
          </button>
        ))}
      </PickerButton>
    </div>
  )
}

/** Trigger + anchored popover shared by the three pickers. */
function PickerButton({
  label,
  aria,
  disabled,
  expanded,
  onClick,
  children
}: {
  label: string
  aria: string
  disabled: boolean
  expanded: boolean
  onClick: () => void
  children: ReactNode
}): JSX.Element {
  return (
    <div className="ark-composer__pick">
      <Button
        variant="ghost"
        size="sm"
        className="ark-composer__pick-trigger"
        aria-haspopup="menu"
        aria-expanded={expanded}
        aria-label={aria}
        disabled={disabled}
        onClick={onClick}
      >
        <span className="ark-composer__pick-label">{label}</span>
        <span className="ark-composer__pick-caret" aria-hidden="true">
          ▾
        </span>
      </Button>
      {expanded ? (
        <div className="ark-composer__popover" role="menu" aria-label={aria}>
          {children}
        </div>
      ) : null}
    </div>
  )
}

function Field({ label, children }: { label: string; children: ReactNode }): JSX.Element {
  return (
    <label className="ark-composer__field">
      <span>{label}</span>
      {children}
    </label>
  )
}

/**
 * Model override input. Committing happens on Enter or blur: PATCH on every
 * keystroke would spam the thread API while the id is half-typed.
 */
function ModelField({
  value,
  options,
  onCommit,
  label
}: {
  value: string
  options: string[]
  onCommit: (value: string) => void
  label: string
}): JSX.Element {
  const [draft, setDraft] = useState(value)
  useEffect(() => setDraft(value), [value])

  const commit = (): void => {
    // An empty override would replace the resolved model with "" server-side;
    // only non-empty values are sent, clearing stays impossible by design.
    if (draft.trim() && draft !== value) onCommit(draft.trim())
  }

  return (
    <Field label={label}>
      <input
        list="ark-composer-model-options"
        value={draft}
        placeholder={options[0] ?? ''}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === 'Enter') {
            event.preventDefault()
            commit()
          }
        }}
      />
      <datalist id="ark-composer-model-options">
        {options.map((id) => (
          <option key={id} value={id} />
        ))}
      </datalist>
    </Field>
  )
}
