/**
 * View switch between the Code workbench and the Settings page.
 *
 * Deliberately not a router: two views, one piece of state, no URL coupling.
 * Default is `code`, so the existing screen is unchanged on first paint.
 */
import { create } from 'zustand'

export type AppView = 'code' | 'settings'

export const APP_VIEWS: AppView[] = ['code', 'settings']

export const APP_VIEW_LABELS: Record<AppView, string> = {
  code: 'Code',
  settings: '设置'
}

export type ViewSlice = {
  view: AppView
  setView: (view: AppView) => void
}

/** A module-level slice keeps the view out of the code-session store. */
export const useViewStore = create<ViewSlice>()((set) => ({
  view: 'code',
  setView: (view) => set({ view })
}))
