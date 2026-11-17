/**
 * Shared two-level "provider -> model" picker, mirroring the
 * FloatingComposerModelPicker / CaseModelPicker: ONE trigger opens a menu whose
 * left column lists providers (hover/focus switches the active one) and whose
 * right column lists only that provider's models. Picking a model reports the
 * (providerId, model) pair together, so no surface ever renders a separate
 * provider box and model box again.
 *
 * The component owns no provider business logic: grouping and "what is current"
 * live in ./groups.ts and are unit-testable without a DOM.
 */
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { useI18n } from '../i18n'
import {
  resolveModelProviderSelection,
  type ModelProviderGroup,
  type ModelProviderSelection
} from './groups'
import './model-provider-menu.css'

export type ModelProviderClear = {
  label: string
  /** Restore the "no override" state (blank provider + model in a form). */
  onClear: () => void
}

export type ModelProviderMenuProps = {
  /** id on the trigger, so a settings `Field` can point `htmlFor` at it. */
  id?: string
  groups: readonly ModelProviderGroup[]
  /** Explicit current selection; a blank providerId means "follow default". */
  value: ModelProviderSelection
  /** Effective runtime default, shown as a preview when `value` is blank. */
  fallback?: ModelProviderSelection
  /** Fired when a model row is picked: both ids are delivered atomically. */
  onSelect: (selection: ModelProviderSelection) => void
  disabled?: boolean
  ariaLabel: string
  placeholder?: string
  /** Custom trigger content (e.g. the composer pill's label + effort + caret). */
  trigger?: ReactNode
  triggerClassName?: string
  menuClassName?: string
  /** Extra class on the container; composer passes its toolbar pick classes. */
  className?: string
  /** Optional controlled open state (the composer also reacts to open/close). */
  open?: boolean
  onOpenChange?: (open: boolean) => void
  /** Title override for the trigger (the composer builds its own pill text). */
  title?: string
  /** Content above the two columns (e.g. a vision warning). */
  header?: ReactNode
  /** Content below the two columns. A function also receives the close action. */
  footer?: ReactNode | ((close: () => void) => ReactNode)
  /** Optional "follow default" entry at the top of the left column. */
  clearSelection?: ModelProviderClear
  /**
   * `columns` (default) keeps two side-by-side columns for settings forms.
   * `flyout` mirrors the session composer: one provider column, and
   * hovering/focusing a provider floats its models as a separate scrollable
   * submenu card beside the menu.
   */
  variant?: 'columns' | 'flyout'
}

/**
 * A stored selection can name a provider/model the current list no longer has
 * (provider deleted, model dropped, or a hand-typed id). Carry it into the menu
 * as a synthetic row so opening the picker never silently discards the value.
 */
function buildDisplayGroups(
  groups: readonly ModelProviderGroup[],
  value: ModelProviderSelection
): ModelProviderGroup[] {
  const out = groups.map((group) => ({ ...group }))
  const providerId = value.providerId.trim()
  const model = value.model.trim()
  if (!providerId || !model) return out
  const existing = out.find((group) => group.id === providerId)
  if (!existing) {
    return [...out, { id: providerId, label: providerId, models: [model] }]
  }
  if (!existing.models.includes(model)) {
    return out.map((group) =>
      group.id === providerId ? { ...group, models: [...group.models, model] } : group
    )
  }
  return out
}

export function ModelProviderMenu({
  id,
  groups,
  value,
  fallback,
  onSelect,
  disabled = false,
  ariaLabel,
  placeholder,
  trigger,
  triggerClassName,
  menuClassName,
  className,
  header,
  footer,
  clearSelection,
  variant = 'columns',
  open: controlledOpen,
  onOpenChange,
  title
}: ModelProviderMenuProps): JSX.Element {
  const { t } = useI18n()
  const flyout = variant === 'flyout'
  const [internalOpen, setInternalOpen] = useState(false)
  const isControlled = controlledOpen !== undefined
  const open = isControlled ? controlledOpen : internalOpen
  const [activeId, setActiveId] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const rootRef = useRef<HTMLDivElement | null>(null)

  const setOpen = (next: boolean): void => {
    if (!next) {
      setQuery('')
      // Next open starts with the submenu collapsed; the re-anchor effect
      // below reopens the stored selection's provider once the menu is open.
      if (flyout) setActiveId(null)
    }
    if (!isControlled) setInternalOpen(next)
    onOpenChange?.(next)
  }

  const displayGroups = useMemo(() => buildDisplayGroups(groups, value), [groups, value])
  const resolved = useMemo(
    () => resolveModelProviderSelection(displayGroups, value, fallback),
    [displayGroups, value, fallback]
  )

  // Re-anchor on the stored selection when it changes (e.g. a runtime fetch
  // resolves after the menu opened). Primitive deps keep an unrelated parent
  // re-render from resetting a provider the operator just hovered over.
  useEffect(() => {
    if (open && resolved.group) setActiveId(resolved.group.id)
  }, [open, resolved.group?.id])

  // On open, and while an async provider list arrives, ensure some group is
  // active without ever overriding a provider already picked by the operator.
  useEffect(() => {
    if (!open || flyout) return
    setActiveId((current) => current ?? displayGroups[0]?.id ?? null)
  }, [open, displayGroups, flyout])

  // Outside-click and Escape close the menu in both modes: `setOpen` routes a
  // controlled menu's close through `onOpenChange` instead of local state.
  useEffect(() => {
    if (!open) return
    const onDown = (event: MouseEvent): void => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) setOpen(false)
    }
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  // Search narrows the model column; a provider whose name matches keeps all
  // of its models, so typing a provider id still shows the full second level.
  const needle = query.trim().toLowerCase()
  const visibleGroups = displayGroups
    .map((group) => {
      const providerMatch = !needle
        || group.label.toLowerCase().includes(needle)
        || group.id.toLowerCase().includes(needle)
      return {
        ...group,
        models: providerMatch
          ? group.models
          : group.models.filter((model) => model.toLowerCase().includes(needle))
      }
    })
    .filter((group) => group.models.length > 0)
  // In flyout mode the submenu stays collapsed until a provider is hovered,
  // focused or clicked (or the stored selection is re-anchored on open).
  const activeGroup = visibleGroups.find((group) => group.id === activeId)
    ?? (flyout ? null : visibleGroups[0])
    ?? null
  const triggerLabel = resolved.group
    ? [resolved.group.label, resolved.model].filter(Boolean).join(' / ')
    : ''
  const shown = title ?? (triggerLabel || placeholder || t('modelProviderMenu.placeholder'))

  const select = (providerId: string, model: string): void => {
    onSelect({ providerId, model })
    setOpen(false)
  }

  // Shared by the inline column and the flyout submenu: exactly the active
  // provider's models, or a translated empty state. The flyout gives this list
  // its own scroll container, so a long inventory stays fully selectable.
  const modelItems: ReactNode = activeGroup && activeGroup.models.length > 0
    ? activeGroup.models.map((model) => {
        const selected = resolved.group?.id === activeGroup.id && resolved.model === model
        return (
          <button
            key={`${activeGroup.id}:${model}`}
            type="button"
            role="menuitemradio"
            aria-checked={selected}
            className={`ark-mpm__item ark-mpm__model${selected ? ' ark-mpm__item--selected' : ''}`}
            title={model}
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => select(activeGroup.id, model)}
          >
            <span className="ark-mpm__item-label">{model}</span>
            {selected ? <span className="ark-mpm__check" aria-hidden="true">✓</span> : null}
          </button>
        )
      })
    : <p className="ark-mpm__empty">{t('modelProviderMenu.noModels')}</p>

  return (
    <div className={`ark-mpm${className ? ` ${className}` : ''}`} ref={rootRef}>
      <button
        id={id}
        type="button"
        className={triggerClassName ?? 'ark-mpm__trigger'}
        disabled={disabled}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={ariaLabel}
        title={shown}
        onClick={() => setOpen(!open)}
      >
        {trigger ?? (
          <>
            <span className="ark-mpm__label">{shown}</span>
            {resolved.isDefault && resolved.group ? (
              <span className="ark-mpm__default-badge">{t('modelProviderMenu.defaultBadge')}</span>
            ) : null}
            <span className="ark-mpm__caret" aria-hidden="true">▾</span>
          </>
        )}
      </button>
      {open ? (
        <div
          className={`ark-mpm__menu${flyout ? ' ark-mpm__menu--flyout' : ''}${menuClassName ? ` ${menuClassName}` : ''}`}
          role="menu"
          aria-label={ariaLabel}
        >
          <label className="ark-mpm__search">
            <input
              type="search"
              value={query}
              placeholder={t('modelProviderMenu.search')}
              aria-label={t('modelProviderMenu.search')}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Escape' && query) {
                  event.stopPropagation()
                  setQuery('')
                }
              }}
            />
          </label>
          {header ? <div className="ark-mpm__header">{header}</div> : null}
          <div className="ark-mpm__columns">
            <div className="ark-mpm__left" role="group" aria-label={t('modelProviderMenu.providerColumn')}>
              {clearSelection ? (
                <button
                  type="button"
                  role="menuitem"
                  className="ark-mpm__item ark-mpm__item--clear"
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => {
                    clearSelection.onClear()
                    setOpen(false)
                  }}
                >
                  <span className="ark-mpm__item-label">{clearSelection.label}</span>
                </button>
              ) : null}
              {visibleGroups.length === 0 ? (
                <p className="ark-mpm__empty">
                  {needle ? t('modelProviderMenu.noMatches') : t('modelProviderMenu.noProviders')}
                </p>
              ) : (
                visibleGroups.map((group) => (
                  <button
                    key={group.id}
                    type="button"
                    role="menuitem"
                    aria-haspopup="menu"
                    aria-expanded={activeGroup?.id === group.id}
                    className={`ark-mpm__item ark-mpm__provider${
                      activeGroup?.id === group.id ? ' ark-mpm__item--active' : ''
                    }${resolved.group?.id === group.id ? ' ark-mpm__item--selected' : ''}`}
                    title={`${group.label} · ${group.id}`}
                    onMouseDown={(event) => event.preventDefault()}
                    onMouseEnter={() => setActiveId(group.id)}
                    onFocus={() => setActiveId(group.id)}
                    onClick={() => setActiveId(group.id)}
                  >
                    <span className="ark-mpm__item-label">{group.label}</span>
                    <span className="ark-mpm__chevron" aria-hidden="true">›</span>
                  </button>
                ))
              )}
            </div>
            {flyout ? (
              activeGroup ? (
                <div
                  className="ark-mpm__right ark-mpm__flyout"
                  role="group"
                  aria-label={`${activeGroup.label} · ${t('modelProviderMenu.modelColumn')}`}
                >
                  <p className="ark-mpm__flyout-title">{activeGroup.label}</p>
                  {modelItems}
                </div>
              ) : null
            ) : (
              <div className="ark-mpm__right" role="group" aria-label={t('modelProviderMenu.modelColumn')}>
                {modelItems}
              </div>
            )}
          </div>
          {footer ? (
            <div className="ark-mpm__footer">
              {typeof footer === 'function' ? footer(() => setOpen(false)) : footer}
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}
