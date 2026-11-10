/**
 * Pure mapping for the "pick an existing project" composer menu.
 *
 * Paths are canonical descriptors from `GET /v1/workspace`; the browser never
 * probes the filesystem or compares paths beyond a case-sensitive dedupe. The
 * thread override is the current root, the host active root and bounded recents
 * are the choices, and adding a project is a separate native-dialog action.
 */
import type { WorkspaceDescriptor } from '../../../shared/contracts'

export type WorkspaceOption = {
  path: string
  label: string
  /** Parent folder, shown muted to disambiguate same-named projects. */
  parent: string
}

export function workspaceBasename(path: string): string {
  const parts = path.trim().replace(/[\\/]+$/, '').split(/[\\/]/).filter(Boolean)
  return parts[parts.length - 1] ?? path.trim()
}

function parentLabel(path: string, label: string): string {
  const parts = path.trim().replace(/[\\/]+$/, '').split(/[\\/]/).filter(Boolean)
  const parent = parts.length >= 2 ? parts[parts.length - 2] ?? '' : ''
  return parent && parent.toLowerCase() !== label.toLowerCase() ? parent : ''
}

export function workspaceOptions(input: {
  override?: string
  active?: WorkspaceDescriptor
  recent: readonly WorkspaceDescriptor[]
}): { current: string; options: WorkspaceOption[] } {
  const current = (input.override?.trim() || input.active?.path || '').trim()
  const paths = [current, input.active?.path ?? '', ...input.recent.map((entry) => entry.path)]
  const seen = new Set<string>()
  const options: WorkspaceOption[] = []
  for (const raw of paths) {
    const path = raw.trim()
    if (!path || seen.has(path)) continue
    seen.add(path)
    const label = workspaceBasename(path)
    options.push({ path, label, parent: parentLabel(path, label) })
  }
  return {
    current,
    options: options.sort((left, right) => {
      if (left.path === current) return -1
      if (right.path === current) return 1
      return left.label.localeCompare(right.label)
    })
  }
}
