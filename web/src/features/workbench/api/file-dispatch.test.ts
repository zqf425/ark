import { describe, expect, it } from 'vitest'
import { previewModeOf } from './file-dispatch'

describe('previewModeOf', () => {
  it('renders markdown extensions as rich text', () => {
    expect(previewModeOf('README.md', false)).toBe('markdown')
    expect(previewModeOf('docs/guide.markdown', false)).toBe('markdown')
    expect(previewModeOf('Notes.MD', false)).toBe('markdown')
  })

  it('keeps code highlighting for other text files', () => {
    expect(previewModeOf('src/main.rs', false)).toBe('code')
    expect(previewModeOf('package.json', false)).toBe('code')
    expect(previewModeOf('md', false)).toBe('code')
  })

  it('binary wins over the extension', () => {
    expect(previewModeOf('notes.md', true)).toBe('binary')
  })
})
