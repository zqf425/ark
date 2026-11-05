import { describe, expect, it } from 'vitest'
import { createStore } from 'zustand/vanilla'
import { createWorkbenchSlice, type WorkbenchSlice } from './workbench-slice'

function makeStore() {
  return createStore<WorkbenchSlice>()((...args) => createWorkbenchSlice(...args))
}

describe('workbench inspector', () => {
  it('starts expanded so task context is resident next to the conversation', () => {
    expect(makeStore().getState().rightCollapsed).toBe(false)
  })

  it('opens when a context view is selected', () => {
    const store = makeStore()
    store.getState().toggleRight()
    expect(store.getState().rightCollapsed).toBe(true)

    store.getState().setActivePanel('plan')
    expect(store.getState().activePanel).toBe('plan')
    expect(store.getState().rightCollapsed).toBe(false)
  })
})
