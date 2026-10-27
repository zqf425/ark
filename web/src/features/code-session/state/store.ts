/**
 * Store composition root for the Code screen.
 *
 * Slices split by concern (design §7.10.2): threads, items/stream, runtime,
 * workbench layout. Components subscribe to a slice and never mutate directly.
 */
import { create } from 'zustand'
import { createItemsSlice, type ItemsSlice } from './items-slice'
import { createRuntimeSlice, type RuntimeSlice } from './runtime-slice'
import { createThreadsSlice, type ThreadsSlice } from './threads-slice'
import { createWorkbenchSlice, type WorkbenchSlice } from './workbench-slice'

export type CodeSessionStore = ThreadsSlice & ItemsSlice & RuntimeSlice & WorkbenchSlice

export const useCodeSessionStore = create<CodeSessionStore>()((...args) => ({
  ...createThreadsSlice(...args),
  ...createItemsSlice(...args),
  ...createRuntimeSlice(...args),
  ...createWorkbenchSlice(...args)
}))

export type { ItemsSlice, RuntimeSlice, ThreadsSlice, WorkbenchSlice }
