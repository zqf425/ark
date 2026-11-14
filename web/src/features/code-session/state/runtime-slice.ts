import type { StateCreator } from 'zustand'
import { loadPersistedDrafts, persistDrafts } from './draft-storage'

export type RuntimeSlice = {
  online: 'unknown' | 'online' | 'offline'
  healthVersion: string | null
  runtimeVersion: string | null
  dataDir: string | null
  threadCount: number | null
  checkedAt: number | null
  healthError: string | null
  sending: boolean
  interrupting: boolean
  commandError: string | null
  /** Unsent composer drafts keyed by thread id (survive thread switches). */
  drafts: Record<string, string>
  /** Restore the persisted draft mirror into the store (once at mount). */
  hydrateDrafts: () => void
  setHealth: (payload: { online: boolean; version?: string | null; error?: string | null }) => void
  setRuntimeInfo: (payload: { version: string | null; dataDir: string | null; threadCount: number | null }) => void
  setSending: (sending: boolean) => void
  setInterrupting: (interrupting: boolean) => void
  setCommandError: (message: string | null) => void
  setDraft: (threadId: string, text: string) => void
  clearDraft: (threadId: string) => void
}

export const createRuntimeSlice: StateCreator<RuntimeSlice, [], [], RuntimeSlice> = (set) => ({
  online: 'unknown',
  healthVersion: null,
  runtimeVersion: null,
  dataDir: null,
  threadCount: null,
  checkedAt: null,
  healthError: null,
  sending: false,
  interrupting: false,
  commandError: null,
  drafts: {},

  // The renderer reloads outside the app's control (arkd restart tears the
  // window's connections and the desktop shell reloads, a dev HMR swap does
  // the same, a renderer crash too). To the user that reads as "a session
  // finished and every other composer went empty", so drafts are hydrated
  // from localStorage once at mount and mirrored back on every change.
  hydrateDrafts: () => set({ drafts: loadPersistedDrafts() }),

  setHealth: ({ online, version = null, error = null }) =>
    set({
      online: online ? 'online' : 'offline',
      healthVersion: version ?? null,
      healthError: error,
      checkedAt: Date.now()
    }),

  setRuntimeInfo: ({ version, dataDir, threadCount }) =>
    set({ runtimeVersion: version, dataDir, threadCount }),

  setSending: (sending) => set({ sending }),
  setInterrupting: (interrupting) => set({ interrupting }),
  setCommandError: (message) => set({ commandError: message }),

  // One entry per thread; empty text stores "" so a cleared draft does not
  // resurrect the old content, and the map is pruned when a thread's draft is
  // sent so long sessions do not accumulate stale entries. Every change is
  // mirrored to localStorage so a reload (shell restart, arkd bounce, crash)
  // restores each thread's unsent text instead of dropping it.
  setDraft: (threadId, text) =>
    set((state) => {
      const drafts = { ...state.drafts, [threadId]: text }
      persistDrafts(drafts)
      return { drafts }
    }),
  clearDraft: (threadId) =>
    set((state) => {
      if (!(threadId in state.drafts)) return state
      const next = { ...state.drafts }
      delete next[threadId]
      persistDrafts(next)
      return { drafts: next }
    })
})
