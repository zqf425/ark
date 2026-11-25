import type { StateCreator } from 'zustand'
import type { RuntimeEvent, ThreadItem, ThreadRuntimeSnapshot, ThreadUsageResponse } from '../../../shared/contracts'
import type { StreamStatus } from '../api/events-api'
import { applyEvent, emptyItemsState, markApprovalLocally, markUserInputLocally, seedFromItems } from './items-reducer'
import { cancelUsageRequest, createUsageRequest, seedUsage } from './usage'
import { indexRounds, roundsFromTurnUsage } from './rounds'
import type { ItemsSnapshot, LoadStatus, UsageRequest } from './types'

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
  seedItems: (items: ThreadItem[], lastSeq: number, runtime: ThreadRuntimeSnapshot, snapshotAt?: number) => void
  /** Fold one SSE event through the pure reducer. */
  ingestEvent: (event: RuntimeEvent) => void
  resetItems: () => void
  setDetailStatus: (status: LoadStatus, error?: string | null) => void
  setStreamStatus: (status: StreamStatus, error?: string | null) => void
  noteApprovalResolved: (approvalId: string, approved: boolean) => void
  noteUserInputAnswered: (requestId: string, answer: string) => void
  /** Claim the thread's usage snapshot and return its seq/generation ticket. */
  beginThreadUsage: (threadId: string) => UsageRequest
  /** Apply a response only when its ticket still owns this thread projection. */
  noteThreadUsage: (usage: ThreadUsageResponse, request: UsageRequest) => boolean
  /** Release a failed or aborted owner without clearing a newer request. */
  cancelThreadUsage: (request: UsageRequest) => void
  /** Merge per-turn timings monotonically with newer reducer-banked rows. */
  noteThreadRounds: (response: ThreadUsageResponse) => void
}

export const createItemsSlice: StateCreator<ItemsSlice, [], [], ItemsSlice> = (set, get) => ({
  ...emptyItemsState(),
  detailStatus: 'idle',
  detailError: null,
  streamStatus: 'closed',
  streamError: null,
  reloadToken: 0,

  requestStreamReload: () => set((state) => ({ reloadToken: state.reloadToken + 1 })),

  seedItems: (items, lastSeq, runtime, snapshotAt) => set((state) => ({
    ...seedFromItems(items, lastSeq, runtime, snapshotAt, state),
    // A fresh server snapshot invalidates every workbench projection, even
    // when the thread id did not change (reload after rewind, another client).
    workbenchRevision: state.workbenchRevision + 1,
    detailStatus: 'ready',
    detailError: null
  })),

  ingestEvent: (event) => set(applyEvent(get(), event)),

  resetItems: () => set((state) => ({ ...emptyItemsState(), workbenchRevision: state.workbenchRevision + 1, detailStatus: 'idle', detailError: null, streamError: null })),

  setDetailStatus: (status, error = null) => set({ detailStatus: status, detailError: error }),

  setStreamStatus: (status, error) => set((state) => ({
    streamStatus: status,
    // `onStatus('reconnecting')` follows `onError` in the SSE loop. A bare
    // status update must not erase the transport diagnostic; only an explicit
    // argument or a recovered `open` changes it.
    streamError: error !== undefined ? error : status === 'open' ? null : state.streamError
  })),

  noteApprovalResolved: (approvalId, approved) => set(markApprovalLocally(get(), approvalId, approved)),

  noteUserInputAnswered: (requestId, answer) => set(markUserInputLocally(get(), requestId, answer)),

  beginThreadUsage: (threadId) => {
    const request = createUsageRequest(get(), threadId)
    set({ usageRequest: request, usageTail: null })
    return request
  },

  noteThreadUsage: (usage, request) => {
    let accepted = false
    set((state) => {
      const next = seedUsage(state, usage, request)
      accepted = next !== state
      return next
    })
    return accepted
  },

  cancelThreadUsage: (request) => set((state) => cancelUsageRequest(state, request)),

  noteThreadRounds: (response) => set((state) => {
    const rounds = roundsFromTurnUsage(response, state.rounds)
    return { rounds, roundIndex: indexRounds(rounds) }
  })
})
