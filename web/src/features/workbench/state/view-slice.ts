/**
 * App view state for the ARK Console shell (approved proposal A).
 *
 * Still deliberately not a router: one store field, no URL coupling. `code`
 * and `settings` remain the two primary views so the existing settings
 * sections that call `setView('code')` keep working; `review` and `new` are
 * the two additional full-screen scenes. The command palette is a boolean on
 * the same slice so ⌘K works from every scene.
 */
import { create } from 'zustand'
import type { MessageKey } from '../../../shared/i18n'

export type AppView = 'code' | 'settings' | 'review' | 'new'

/** The two views offered by the legacy ViewSwitch (kept for compatibility). */
export const APP_VIEWS: AppView[] = ['code', 'settings']

/** Labels are catalog keys so the switch follows the display language. */
export const APP_VIEW_LABEL_KEYS: Record<AppView, MessageKey> = {
  code: 'workbench.view.code',
  settings: 'workbench.view.settings',
  review: 'console.review.title',
  new: 'console.rail.new'
}

export type ViewSlice = {
  view: AppView
  setView: (view: AppView) => void
  commandOpen: boolean
  setCommandOpen: (open: boolean) => void
  toggleCommand: () => void
}

/** A module-level slice keeps the view out of the code-session store. */
export const useViewStore = create<ViewSlice>()((set) => ({
  view: 'code',
  setView: (view) => set({ view }),
  commandOpen: false,
  setCommandOpen: (commandOpen) => set({ commandOpen }),
  toggleCommand: () => set((state) => ({ commandOpen: !state.commandOpen }))
}))
