/**
 * File-chip data for write-family tool cards (requirements/tool-file-preview.md).
 *
 * Pure so the extraction is testable without React: only tools with a
 * structured `path` argument (`write`, `edit`) produce a chip. Every other
 * tool — including `bash`, which can also touch files — stays null: the chip
 * promises "this exact file was written", so guessing from free-form text
 * would break that promise.
 */
import type { ToolPayload } from '../../shared/contracts'

/** Tools whose args carry a `path` string pointing at the written file. */
const PATH_TOOLS = new Set(['write', 'edit'])

export type ToolFileTarget = {
  /** Full workspace-relative path as passed to the tool; feeds `selectFile`. */
  path: string
  /** File basename for the chip label. */
  name: string
  /** Parent directory hint, '' when the file sits at the workspace root. */
  dir: string
}

export function toolTargetPath(tool: ToolPayload): ToolFileTarget | null {
  if (!PATH_TOOLS.has(tool.tool)) return null
  const args = tool.arguments
  if (args === null || typeof args !== 'object' || Array.isArray(args)) return null
  const raw = (args as Record<string, unknown>).path
  if (typeof raw !== 'string' || raw.trim() === '') return null
  const path = raw.replace(/\\/g, '/')
  const cut = path.lastIndexOf('/')
  return {
    path: raw,
    name: cut >= 0 ? path.slice(cut + 1) : path,
    dir: cut > 0 ? path.slice(0, cut) : ''
  }
}
