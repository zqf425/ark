import { describe, expect, it } from 'vitest'
import { canEditComposer, canSendToThread } from './composer-availability'

describe('canSendToThread', () => {
  it('blocks a new turn until the event stream is ready', () => {
    expect(canSendToThread('thread-1', false, 'reconnecting')).toBe(false)
    expect(canSendToThread('thread-1', false, 'closed')).toBe(false)
  })

  it('requires both a selected thread and an idle send command', () => {
    expect(canSendToThread(null, false, 'open')).toBe(false)
    expect(canSendToThread('thread-1', true, 'open')).toBe(false)
    expect(canSendToThread('thread-1', false, 'open')).toBe(true)
  })

  it('keeps the draft editable while the event stream reconnects', () => {
    expect(canEditComposer('thread-1', false)).toBe(true)
    expect(canEditComposer('thread-1', true)).toBe(false)
    expect(canEditComposer(null, false)).toBe(false)
  })
})
