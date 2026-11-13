import type { StateCreator } from 'zustand'

/** Right-hand panel slots the Code page exposes (design §7.10.2 workbench). */
export type PanelId = 'changes' | 'plan' | 'file' | 'terminal'

export const PANEL_IDS: PanelId[] = ['changes', 'plan', 'file', 'terminal']

/** Purely presentational layout state; no business semantics. */
export type WorkbenchSlice = {
  activePanel: PanelId
  leftWidth: number
  leftCollapsed: boolean
  rightCollapsed: boolean
  selectedFilePath: string | null
  setActivePanel: (panel: PanelId) => void
  /** Explicit "open this context view" intent: selects the tab AND expands. */
  openPanel: (panel: PanelId) => void
  setLeftWidth: (width: number) => void
  toggleLeft: () => void
  toggleRight: () => void
  selectFile: (path: string | null) => void
}

const MIN_SIDE = 200
const MAX_SIDE = 640
const DEFAULT_LEFT = 272

export function clampSideWidth(width: number): number {
  if (!Number.isFinite(width)) return DEFAULT_LEFT
  return Math.min(MAX_SIDE, Math.max(MIN_SIDE, Math.round(width)))
}

export const createWorkbenchSlice: StateCreator<WorkbenchSlice, [], [], WorkbenchSlice> = (set) => ({
  activePanel: 'changes',
  leftWidth: DEFAULT_LEFT,
  leftCollapsed: false,
  // The inspector starts closed: it is an on-demand context column, so the
  // conversation gets the full canvas until the user asks for the panels.
  rightCollapsed: true,
  selectedFilePath: null,

  // Selecting a tab is a layout change *inside* the inspector. It must never
  // reopen a drawer the user closed — only `openPanel`/`toggleRight` do that.
  setActivePanel: (panel) => set({ activePanel: panel }),
  openPanel: (panel) => set({ activePanel: panel, rightCollapsed: false }),
  setLeftWidth: (width) => set({ leftWidth: clampSideWidth(width) }),
  toggleLeft: () => set((state) => ({ leftCollapsed: !state.leftCollapsed })),
  toggleRight: () => set((state) => ({ rightCollapsed: !state.rightCollapsed })),
  // Picking a file switches the tab only. The row can only be clicked while the
  // inspector is already open, so forcing `rightCollapsed: false` here bought
  // nothing and made a closed drawer spring back open.
  selectFile: (path) => set(path ? { selectedFilePath: path, activePanel: 'file' } : { selectedFilePath: null })
})
