import { describe, expect, it } from 'vitest'
import type { DiffLine } from '../workbench/panels/diff-lines'
import { buildSplitRows, tokenize } from './review-diff'

const line = (
  type: DiffLine['type'], text: string,
  oldNumber: number | null = null, newNumber: number | null = null
): DiffLine => ({ type, text, oldNumber, newNumber })

describe('buildSplitRows', () => {
  it('mirrors context lines to both columns', () => {
    const first = buildSplitRows([line('context', 'same')])[0]
    expect(first?.kind).toBe('pair')
    if (first && first.kind === 'pair') {
      expect(first.left?.text).toBe('same')
      expect(first.right?.text).toBe('same')
    }
  })

  it('pairs a remove/add run on one shared row index and pads the short side', () => {
    const rows = buildSplitRows([
      line('remove', 'a', 1, null), line('add', 'b', null, 1),
      line('add', 'c', null, 2), line('context', 'tail', 2, 3)
    ])
    expect(rows).toHaveLength(3)
    const first = rows[0]
    const second = rows[1]
    if (first && first.kind === 'pair') {
      expect(first.left?.text).toBe('a')
      expect(first.right?.text).toBe('b')
    }
    if (second && second.kind === 'pair') {
      expect(second.left).toBeNull()
      expect(second.right?.text).toBe('c')
    }
  })

  it('flushes pending runs before a hunk header and makes it a meta row', () => {
    const rows = buildSplitRows([line('add', 'new', null, 5), line('meta', '@@ -1 +1 @@')])
    expect(rows[0]?.kind).toBe('pair')
    expect(rows[1]).toEqual({ kind: 'meta', text: '@@ -1 +1 @@' })
  })

  it('flushes a trailing add run', () => {
    const rows = buildSplitRows([line('context', 'x'), line('add', 'y', null, 2)])
    const last = rows[1]
    expect(rows).toHaveLength(2)
    if (last && last.kind === 'pair') expect(last.right?.text).toBe('y')
  })
})

describe('tokenize', () => {
  it('classifies keywords, numbers and comments without losing any text', () => {
    const source = '    let n = 3; // note'
    const tokens = tokenize(source)
    expect(tokens.map((token) => token.text).join('')).toBe(source)
    expect(tokens.some((token) => token.cls === 'tk-kw' && token.text === 'let')).toBe(true)
    expect(tokens.some((token) => token.cls === 'tk-num' && token.text === '3')).toBe(true)
    expect(tokens.some((token) => token.cls === 'tk-cm')).toBe(true)
  })
})
