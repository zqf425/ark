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

  setActiveThread: (threadId) => set({ activeThreadId: threadId }),

  setListStatus: (status, error = null) => set({ listStatus: status, listError: error }),

  removeThread: (threadId) =>
    set((state) => ({
      threads: state.threads.filter((row) => row.thread_id !== threadId),
      activeThreadId: state.activeThreadId === threadId ? null : state.activeThreadId
    }))
})
