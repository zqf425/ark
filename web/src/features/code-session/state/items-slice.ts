import type { StateCreator } from 'zustand'
import type { RuntimeEvent, ThreadItem, ThreadRuntimeSnapshot, ThreadUsageResponse } from '../../../shared/contracts'
import type { StreamStatus } from '../api/events-api'
import { applyEvent, emptyItemsState, markApprovalLocally, markUserInputLocally, seedFromItems } from './items-reducer'
import { seedUsage } from './usage'
import type { ChildCompletionNotice, ItemsSnapshot, LoadStatus } from './types'

export type ItemsSlice = ItemsSnapshot & {
  detailStatus: LoadStatus
  detailError: string | null
  streamStatus: StreamStatus
  streamError: string | null
  /**
   * Bumped to force the stream hook to re-fetch the detail and re-subscribe SSE
   * from the new watermark (e.g. after rewind physically truncates history).
   */
  reloadToken: number
  requestStreamReload: () => void
  /** Replace the stream with the authoritative server snapshot. */
  seedItems: (items: ThreadItem[], lastSeq: number, childCompletions: ChildCompletionNotice[], runtime: ThreadRuntimeSnapshot) => void
  /** Fold one SSE event through the pure reducer. */
  ingestEvent: (event: RuntimeEvent) => void
  resetItems: () => void
  setDetailStatus: (status: LoadStatus, error?: string | null) => void
  setStreamStatus: (status: StreamStatus, error?: string | null) => void
  noteApprovalResolved: (approvalId: string, approved: boolean) => void
  noteUserInputAnswered: (requestId: string, answer: string) => void
  /** Seed the usage aggregate from the thread usage endpoint (idempotent). */
  noteThreadUsage: (usage: ThreadUsageResponse) => void
}

export const createItemsSlice: StateCreator<ItemsSlice, [], [], ItemsSlice> = (set, get) => ({
  ...emptyItemsState(),
  detailStatus: 'idle',
  detailError: null,
  streamStatus: 'closed',
  streamError: null,
  reloadToken: 0,

  requestStreamReload: () => set((state) => ({ reloadToken: state.reloadToken + 1 })),

  seedItems: (items, lastSeq, childCompletions, runtime) => set({
    ...seedFromItems(items, lastSeq, childCompletions, runtime),
    detailStatus: 'ready',
    detailError: null
  }),

  ingestEvent: (event) => set(applyEvent(get(), event)),

  resetItems: () => set({ ...emptyItemsState(), detailStatus: 'idle', detailError: null, streamError: null }),

  setDetailStatus: (status, error = null) => set({ detailStatus: status, detailError: error }),

  setStreamStatus: (status, error = null) => set({ streamStatus: status, streamError: error }),

  noteApprovalResolved: (approvalId, approved) => set(markApprovalLocally(get(), approvalId, approved)),

  noteUserInputAnswered: (requestId, answer) => set(markUserInputLocally(get(), requestId, answer)),

  noteThreadUsage: (usage) => set(seedUsage(get(), usage))
})
