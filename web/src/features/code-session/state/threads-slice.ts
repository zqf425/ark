import type { StateCreator } from 'zustand'
import type { ThreadId, ThreadSummary } from '../../../shared/contracts'
import type { LoadStatus } from './types'

export type ThreadsSlice = {
  threads: ThreadSummary[]
  activeThreadId: ThreadId | null
  listStatus: LoadStatus
  listError: string | null
  setThreads: (threads: ThreadSummary[]) => void
  upsertThread: (thread: ThreadSummary) => void
  /** Monotonically advance one row's durable watermark from the live stream. */
  noteThreadSeq: (threadId: ThreadId, seq: number) => void
  /**
   * Fold a `thread_updated` rename into one rail row without touching its
   * other fields or the row order. The list endpoint remains authoritative on
   * refresh; this only keeps the title current between refreshes.
   */
  renameThreadRow: (threadId: ThreadId, title: string) => void
  setActiveThread: (threadId: ThreadId | null) => void
  setListStatus: (status: LoadStatus, error?: string | null) => void
  removeThread: (threadId: ThreadId) => void
}

export const createThreadsSlice: StateCreator<ThreadsSlice, [], [], ThreadsSlice> = (set, get) => ({
  threads: [],
  activeThreadId: null,
  listStatus: 'idle',
  listError: null,

  setThreads: (threads) => set({ threads, listStatus: 'ready', listError: null }),

  upsertThread: (thread) => {
    const existing = get().threads
    const index = existing.findIndex((row) => row.thread_id === thread.thread_id)
    if (index === -1) {
      // Newest first mirrors the runtime's `updated_at` desc ordering.
      set({ threads: [thread, ...existing] })
      return
    }
    const next = [...existing]
    next[index] = thread
    set({ threads: next })
  },

  /**
   * Keep the rail's `last_seq` current without reordering rows. The list
   * endpoint stays authoritative on refresh; this only avoids showing a stale
   * number, and ignores stale/duplicate frames so it never rewinds the rail.
   */
  noteThreadSeq: (threadId, seq) => set((state) => {
    const index = state.threads.findIndex((row) => row.thread_id === threadId)
    if (index === -1) return {}
    const row = state.threads[index]!
    if (typeof row.last_seq === 'number' && row.last_seq >= seq) return {}
    const next = [...state.threads]
    next[index] = { ...row, last_seq: seq }
    return { threads: next }
  }),

  setActiveThread: (threadId) => set({ activeThreadId: threadId }),

  renameThreadRow: (threadId, title) => set((state) => {
    const next = title.trim()
    const index = next ? state.threads.findIndex((row) => row.thread_id === threadId) : -1
    if (index === -1) return {}
    const rows = [...state.threads]
    rows[index] = { ...rows[index]!, title: next }
    return { threads: rows }
  }),

  setListStatus: (status, error = null) => set({ listStatus: status, listError: error }),

  removeThread: (threadId) =>
    set((state) => ({
      threads: state.threads.filter((row) => row.thread_id !== threadId),
      activeThreadId: state.activeThreadId === threadId ? null : state.activeThreadId
    }))
})
