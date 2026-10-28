import { describe, expect, it } from 'vitest'
import { createStore } from 'zustand/vanilla'
import { createWorkbenchSlice, type WorkbenchSlice } from './workbench-slice'

function makeStore() {
  return createStore<WorkbenchSlice>()((...args) => createWorkbenchSlice(...args))
}

describe('workbench context drawer', () => {
  it('starts collapsed so an idle task keeps the conversation canvas primary', () => {
    expect(makeStore().getState().rightCollapsed).toBe(true)
  })

  it('opens when a context view is selected', () => {
    const store = makeStore()
    store.getState().setActivePanel('plan')

    expect(store.getState().activePanel).toBe('plan')
    expect(store.getState().rightCollapsed).toBe(false)
  })
})
