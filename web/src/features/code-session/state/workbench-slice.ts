import type { StateCreator } from 'zustand'

/** Right-hand panel slots the Code page exposes (design §7.10.2 workbench). */
export type PanelId = 'changes' | 'plan' | 'file' | 'terminal'

export const PANEL_IDS: PanelId[] = ['changes', 'plan', 'file', 'terminal']

/** Purely presentational layout state; no business semantics. */
export type WorkbenchSlice = {
  activePanel: PanelId
  rightWidth: number
  leftWidth: number
  leftCollapsed: boolean
  setActivePanel: (panel: PanelId) => void
  setRightWidth: (width: number) => void
  setLeftWidth: (width: number) => void
  toggleLeft: () => void
}

const MIN_SIDE = 200
const MAX_SIDE = 640
const DEFAULT_LEFT = 272
const DEFAULT_RIGHT = 400

export function clampSideWidth(width: number): number {
  if (!Number.isFinite(width)) return DEFAULT_RIGHT
  return Math.min(MAX_SIDE, Math.max(MIN_SIDE, Math.round(width)))
}

export const createWorkbenchSlice: StateCreator<WorkbenchSlice, [], [], WorkbenchSlice> = (set) => ({
  activePanel: 'changes',
  rightWidth: DEFAULT_RIGHT,
  leftWidth: DEFAULT_LEFT,
  leftCollapsed: false,

  setActivePanel: (panel) => set({ activePanel: panel }),
  setRightWidth: (width) => set({ rightWidth: clampSideWidth(width) }),
  setLeftWidth: (width) => set({ leftWidth: clampSideWidth(width) }),
  toggleLeft: () => set((state) => ({ leftCollapsed: !state.leftCollapsed }))
})
