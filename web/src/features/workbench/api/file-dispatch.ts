/**
 * Preview mode dispatch for one path (requirements/workspace-file-preview.md).
 *
 * Pure so the decision is testable without React: Markdown renders as rich
 * text through the shared `<Markdown>` pipeline, everything else keeps the
 * syntax-highlighted source view. The list is deliberately short — adding a
 * format here means committing to its renderer, not just an extension string.
 */
export type PreviewMode = 'markdown' | 'code' | 'binary'

export function previewModeOf(path: string, binary: boolean): PreviewMode {
  if (binary) return 'binary'
  const lower = path.toLowerCase()
  if (lower.endsWith('.md') || lower.endsWith('.markdown')) return 'markdown'
  return 'code'
}
