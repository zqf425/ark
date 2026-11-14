/**
 * Pure grouping for the thread rail's project view (对齐 smart-ark：左侧列表
 * 先按会话的**实际生效**工作空间聚合成项目，再列会话)。
 *
 * 分组键优先取 `ThreadSummary.workspace`——`GET /v1/threads` 行内已携带的解析后
 * 工作空间覆盖；未设置覆盖的会话回退到运行时当前生效的工作空间（active root，
 * 与回合同一解析顺序，见 arkd `resolve_workspace`），仍然无主时才落进一个
 * "默认项目"。前端不再逐行拉 detail 或猜测路径。
 */
import type { ThreadSummary } from '../../../shared/contracts'

export type ThreadProject = {
  /** Workspace path as sent by the runtime; the stable grouping key. */
  path: string
  /** Display name: the workspace folder's basename. */
  label: string
  /** Parent folder, shown muted to disambiguate same-named projects. */
  parent: string
  /** Threads in this project, runtime order (updated_at desc) preserved. */
  threads: ThreadSummary[]
}

/** Key for threads without an explicit workspace override. */
export const DEFAULT_PROJECT_PATH = ''

export function workspaceBasename(path: string): string {
  const parts = path.trim().replace(/[\\/]+$/, '').split(/[\\/]/).filter(Boolean)
  return parts[parts.length - 1] ?? path.trim()
}

function parentLabel(path: string, label: string): string {
  const parts = path.trim().replace(/[\\/]+$/, '').split(/[\\/]/).filter(Boolean)
  const parent = parts.length >= 2 ? parts[parts.length - 2] ?? '' : ''
  return parent && parent.toLowerCase() !== label.toLowerCase() ? parent : ''
}

/**
 * Group threads by their effective workspace. A thread without its own
 * workspace override inherits the runtime's active root (same resolution order
 * as a turn), mirroring smart-ark's sidebar where a session runs inside the
 * currently selected project until it explicitly points elsewhere.
 *
 * Ordering mirrors smart-ark: the active workspace's group comes first — even
 * when it holds no threads, so the "current project" is always visible — and
 * the remaining projects follow in input order, inheriting the runtime's
 * deterministic ordering (the project whose thread was updated most recently
 * first). Projects stay mutually exclusive—one thread belongs to exactly one
 * group.
 */
export function groupThreadsByProject(
  threads: readonly ThreadSummary[],
  activeWorkspace?: string
): { default: ThreadProject; projects: ThreadProject[] } {
  const active = activeWorkspace?.trim() ?? ''
  const buckets = new Map<string, ThreadProject>()
  const defaultProject: ThreadProject = {
    path: DEFAULT_PROJECT_PATH,
    label: '',
    parent: '',
    threads: []
  }
  const ensureProject = (path: string): ThreadProject => {
    let project = buckets.get(path)
    if (!project) {
      const label = workspaceBasename(path)
      project = { path, label, parent: parentLabel(path, label), threads: [] }
      buckets.set(path, project)
    }
    return project
  }
  for (const thread of threads) {
    const path = thread.workspace?.trim() || active
    if (!path) {
      defaultProject.threads.push(thread)
      continue
    }
    ensureProject(path).threads.push(thread)
  }
  const projects = [...buckets.values()]
  if (active) {
    // The active root may hold no thread of its own; still surface its group
    // first so the rail reflects the project the composer is pointed at.
    const activeProject = ensureProject(active)
    const rest = projects.filter((project) => project.path !== active)
    rest.unshift(activeProject)
    return { default: defaultProject, projects: rest }
  }
  return { default: defaultProject, projects }
}

/**
 * Filter helper shared with the search/scope bar: a project group matches when
 * its name/path hits the needle; row matching stays in `filterThreads`.
 */
export function projectMatchesNeedle(project: ThreadProject, needle: string): boolean {
  const normalized = needle.trim().toLocaleLowerCase()
  if (!normalized) return true
  return (
    project.label.toLocaleLowerCase().includes(normalized) ||
    project.path.toLocaleLowerCase().includes(normalized)
  )
}
