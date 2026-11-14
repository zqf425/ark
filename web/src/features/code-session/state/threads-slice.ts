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
  /** Advance one row's last-activity time from the live stream. */
  noteThreadActivity: (threadId: ThreadId, at: string) => void
  /** Set one row's busy flag from the live turn reducer. */
  setThreadRunning: (threadId: ThreadId, running: boolean) => void
  /** Merge a list poll's busy set; rows outside the set read as idle. */
  applyRunningFlags: (running: ThreadId[]) => void
  /**
   * Fold a `thread_updated` rename into one rail row without touching its
   * other fields or the row order. The list endpoint remains authoritative on
   * refresh; this only keeps the title current between refreshes.
   */
  renameThreadRow: (threadId: ThreadId, title: string) => void
  /**
   * Fold a per-session workspace change into one rail row so the project
   * grouping follows a composer switch immediately, without waiting for the
   * next list refresh.
   */
  setThreadWorkspaceRow: (threadId: ThreadId, workspace: string) => void
  setActiveThread: (threadId: ThreadId | null) => void
  setListStatus: (status: LoadStatus, error?: string | null) => void
  removeThread: (threadId: ThreadId) => void
}

/** The wire's activity key with the same fallback the runtime list applies. */
export function activityKeyOf(thread: ThreadSummary): string {
  return thread.last_event_at ?? thread.updated_at
}
/**
 * Server order is authoritative (active, then last activity desc, id last).
 * Re-sorting here keeps a live "just streamed" row at the top between list
 * refreshes, and keeps an stable order for rows whose keys did not change
 * (the comparator ties on the immutable id, so equal keys never swap).
 */
function sortByServerOrder(threads: ThreadSummary[]): ThreadSummary[] {
  return [...threads].sort((a, b) => {
    if (a.archived !== b.archived) return a.archived ? 1 : -1
    const keyDiff = Date.parse(activityKeyOf(b)) - Date.parse(activityKeyOf(a))
    if (keyDiff !== 0) return keyDiff
    return a.thread_id < b.thread_id ? -1 : a.thread_id > b.thread_id ? 1 : 0
  })
}

export const createThreadsSlice: StateCreator<ThreadsSlice, [], [], ThreadsSlice> = (set) => ({
  threads: [],
  activeThreadId: null,
  listStatus: 'idle',
  listError: null,

  setThreads: (threads) => set({ threads: sortByServerOrder(threads), listStatus: 'ready', listError: null }),

  upsertThread: (thread) =>
    set((state) => {
      const index = state.threads.findIndex((row) => row.thread_id === thread.thread_id)
      if (index === -1) {
        return { threads: sortByServerOrder([thread, ...state.threads]) }
      }
      const next = [...state.threads]
      next[index] = thread
      return { threads: sortByServerOrder(next) }
    }),

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

  /**
   * Advance the activity key from the live stream, then restore the server
   * order. Callers only fire this on turn boundaries (the user posted a
   * message or the turn ended), so in-flight events never reorder the rail.
   * A stale or older timestamp is ignored: the list poll stays authoritative.
   */
  noteThreadActivity: (threadId, at) => set((state) => {
    const index = state.threads.findIndex((row) => row.thread_id === threadId)
    if (index === -1) return {}
    const row = state.threads[index]!
    const incoming = Date.parse(at)
    if (Number.isNaN(incoming)) return {}
    const current = Date.parse(activityKeyOf(row))
    if (!Number.isNaN(current) && incoming < current) return {}
    const next = [...state.threads]
    next[index] = { ...row, last_event_at: at }
    return { threads: sortByServerOrder(next) }
  }),

  /**
   * Fold the live turn reducer's verdict into one row. The active thread's
   * spinner is driven from here so it reacts to terminal events in the same
   * frame the canvas settles.
   */
  setThreadRunning: (threadId, running) => set((state) => {
    const index = state.threads.findIndex((row) => row.thread_id === threadId)
    if (index === -1 || state.threads[index]!.running === running) return {}
    const next = [...state.threads]
    next[index] = { ...state.threads[index]!, running }
    return { threads: next }
  }),

  /**
   * Fold one list poll's busy set into the loaded rows: membership is the
   * server's verdict, absence reads as idle. Keeping this pure is what makes
   * a finished session's spinner stop — including background rows the user
   * never re-opens. The only stale-frame hazard is the *active* thread while
   * a turn is in flight, and `use-thread-running.ts` already covers it there
   * by re-adding the active id to the set whenever `activeTurnId` is
   * non-null; the slice must not duplicate that exemption or a settled
   * active row could never clear.
   */
  applyRunningFlags: (running) => set((state) => {
    const runningSet = new Set(running)
    let changed = false
    const next = state.threads.map((row) => {
      const flag = runningSet.has(row.thread_id)
      if (row.running === flag) return row
      changed = true
      return { ...row, running: flag }
    })
    return changed ? { threads: next } : {}
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

  /**
   * Fold a per-session workspace change into one rail row so the project
   * grouping follows the switch immediately, without waiting for the next
   * list refresh (the list endpoint stays authoritative otherwise).
   */
  setThreadWorkspaceRow: (threadId, workspace) => set((state) => {
    const index = state.threads.findIndex((row) => row.thread_id === threadId)
    if (index === -1) return {}
    const rows = [...state.threads]
    rows[index] = { ...rows[index]!, workspace: workspace || undefined }
    return { threads: rows }
  }),

  setListStatus: (status, error = null) => set({ listStatus: status, listError: error }),

  removeThread: (threadId) =>
    set((state) => ({
      threads: state.threads.filter((row) => row.thread_id !== threadId),
      activeThreadId: state.activeThreadId === threadId ? null : state.activeThreadId
    }))
})
