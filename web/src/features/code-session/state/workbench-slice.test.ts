import { describe, expect, it } from 'vitest'
import { createStore } from 'zustand/vanilla'
import { createWorkbenchSlice, type WorkbenchSlice } from './workbench-slice'

function makeStore() {
  return createStore<WorkbenchSlice>()((...args) => createWorkbenchSlice(...args))
}

describe('workbench inspector', () => {
  it('starts collapsed so the conversation owns the canvas', () => {
    expect(makeStore().getState().rightCollapsed).toBe(true)
  })

  it('opens when a context view is explicitly opened', () => {
    const store = makeStore()
    expect(store.getState().rightCollapsed).toBe(true)

    store.getState().openPanel('plan')
    expect(store.getState().activePanel).toBe('plan')
    expect(store.getState().rightCollapsed).toBe(false)
  })

  it('stays closed when a tab is selected after a manual collapse', () => {
    const store = makeStore()
    store.getState().openPanel('changes')
    store.getState().toggleRight()
    expect(store.getState().rightCollapsed).toBe(true)

    // A tab click is a change *inside* the inspector; it must not reopen it.
    store.getState().setActivePanel('plan')
    expect(store.getState().activePanel).toBe('plan')
    expect(store.getState().rightCollapsed).toBe(true)
  })

  it('stays closed when a file is selected after a manual collapse', () => {
    const store = makeStore()
    store.getState().openPanel('changes')
    store.getState().toggleRight()

    store.getState().selectFile('src/main.ts')
    expect(store.getState().selectedFilePath).toBe('src/main.ts')
    expect(store.getState().activePanel).toBe('file')
    expect(store.getState().rightCollapsed).toBe(true)
  })

  it('toggles open and closed', () => {
    const store = makeStore()
    store.getState().toggleRight()
    expect(store.getState().rightCollapsed).toBe(false)
    store.getState().toggleRight()
    expect(store.getState().rightCollapsed).toBe(true)
  })
})
