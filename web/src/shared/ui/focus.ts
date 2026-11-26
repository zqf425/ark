/** Keyboard focus contracts shared by the small menus and confirmation dialog. */
import { useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent, type KeyboardEventHandler, type RefObject } from 'react'

const MENU_ITEM = '[role^="menuitem"]:not(:disabled)'
const FOCUSABLE = 'button:not(:disabled), a[href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])'

function itemsWithin(container: HTMLElement | null, selector: string): HTMLElement[] {
  return container ? [...container.querySelectorAll<HTMLElement>(selector)].filter((node) => node.tabIndex >= 0) : []
}

function moveFocus(items: HTMLElement[], delta: number): void {
  if (items.length === 0) return
  const current = items.indexOf(document.activeElement as HTMLElement)
  const next = current < 0 ? (delta > 0 ? 0 : items.length - 1) : (current + delta + items.length) % items.length
  items[next]?.focus()
}

function activateFocusedMenuItem(event: ReactKeyboardEvent<HTMLElement>, selector: string): boolean {
  const active = document.activeElement
  if (!(active instanceof HTMLElement) || !active.matches(selector)) return false
  event.preventDefault()
  active.click()
  return true
}

/** Roving menu focus; only Escape restores the trigger, while Tab stays native. */
export function useMenuFocus(
  open: boolean,
  onDismiss: () => void,
  triggerRef?: RefObject<HTMLElement | null>
): { ref: RefObject<HTMLDivElement>; onKeyDown: KeyboardEventHandler<HTMLDivElement> } {
  const ref = useRef<HTMLDivElement>(null)
  const dismissRef = useRef(onDismiss)
  const restoreFocus = useRef(false)
  dismissRef.current = onDismiss

  useEffect(() => {
    if (!open) return
    restoreFocus.current = false
    itemsWithin(ref.current, MENU_ITEM)[0]?.focus()
    return () => {
      if (restoreFocus.current) triggerRef?.current?.focus()
    }
  }, [open, triggerRef])

  const onKeyDown: KeyboardEventHandler<HTMLDivElement> = (event) => {
    const items = itemsWithin(ref.current, MENU_ITEM)
    if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      restoreFocus.current = true
      dismissRef.current()
    } else if (event.key === 'Tab') {
      // Re-anchor native navigation at the trigger before the focused menu row
      // disappears; Shift+Tab then naturally walks backwards out of the menu.
      triggerRef?.current?.focus()
      dismissRef.current()
    } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      moveFocus(items, event.key === 'ArrowDown' ? 1 : -1)
    } else if (event.key === 'Home' || event.key === 'End') {
      event.preventDefault()
      const target = event.key === 'Home' ? items[0] : items[items.length - 1]
      target?.focus()
    } else if (event.key === 'Enter' || event.key === ' ') {
      activateFocusedMenuItem(event, MENU_ITEM)
    }
  }
  return { ref, onKeyDown }
}

/** Initial focus, a wrapping Tab trap, Escape, and trigger restoration. */
export function useDialogFocus(
  open: boolean,
  onClose: () => void,
  triggerRef?: RefObject<HTMLElement | null>
): { ref: RefObject<HTMLDivElement>; onKeyDown: KeyboardEventHandler<HTMLDivElement> } {
  const ref = useRef<HTMLDivElement>(null)
  const closeRef = useRef(onClose)
  closeRef.current = onClose

  useEffect(() => {
    if (!open) return
    const fallback = triggerRef?.current ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null)
    const initial = ref.current?.querySelector<HTMLElement>('[data-autofocus]:not(:disabled)') ?? itemsWithin(ref.current, FOCUSABLE)[0]
    if (initial) initial.focus()
    else ref.current?.focus()
    return () => fallback?.focus()
  }, [open, triggerRef])

  const onKeyDown: KeyboardEventHandler<HTMLDivElement> = (event) => {
    if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      closeRef.current()
      return
    }
    if (event.key !== 'Tab') return
    const focusable = itemsWithin(ref.current, FOCUSABLE)
    if (focusable.length === 0) {
      event.preventDefault()
      ref.current?.focus()
      return
    }
    const current = focusable.indexOf(document.activeElement as HTMLElement)
    const delta = event.shiftKey ? -1 : 1
    const next = current < 0 ? (event.shiftKey ? focusable.length - 1 : 0) : (current + delta + focusable.length) % focusable.length
    event.preventDefault()
    focusable[next]?.focus()
  }
  return { ref, onKeyDown }
}
