import { describe, expect, it } from 'vitest'
import type { ThreadSummary } from '../../../shared/contracts'
import {
  DEFAULT_PROJECT_PATH,
  groupThreadsByProject,
  projectMatchesNeedle,
  workspaceBasename
} from './thread-projects'

function row(overrides: Partial<ThreadSummary>): ThreadSummary {
  return {
    thread_id: overrides.thread_id ?? (`t-${Math.random().toString(36).slice(2)}` as ThreadSummary['thread_id']),
    title: 'title',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    archived: false,
    running: false,
    ...overrides
  }
}

describe('workspaceBasename', () => {
  it('takes the last path segment and tolerates trailing separators', () => {
    expect(workspaceBasename('/Users/admin/Desktop/code/ark')).toBe('ark')
    expect(workspaceBasename('D:\\code\\ark\\')).toBe('ark')
    expect(workspaceBasename(' ark ')).toBe('ark')
  })
})

describe('groupThreadsByProject', () => {
  it('groups threads with the same workspace into one project', () => {
    const a1 = row({ thread_id: 'a1' as ThreadSummary['thread_id'], workspace: '/code/ark' })
    const a2 = row({ thread_id: 'a2' as ThreadSummary['thread_id'], workspace: '/code/ark' })
    const b1 = row({ thread_id: 'b1' as ThreadSummary['thread_id'], workspace: '/code/qdc' })
    const grouped = groupThreadsByProject([a1, b1, a2])
    expect(grouped.projects).toHaveLength(2)
    const ark = grouped.projects.find((project) => project.path === '/code/ark')
    expect(ark?.threads.map((thread) => thread.thread_id)).toEqual(['a1', 'a2'])
    expect(ark?.label).toBe('ark')
    expect(ark?.parent).toBe('code')
  })

  it('collects workspace-less threads into the default project', () => {
    const a = row({ thread_id: 'a' as ThreadSummary['thread_id'] })
    const b = row({ thread_id: 'b' as ThreadSummary['thread_id'], workspace: '  ' })
    const grouped = groupThreadsByProject([a, b])
    expect(grouped.default.threads.map((thread) => thread.thread_id)).toEqual(['a', 'b'])
    expect(grouped.projects).toHaveLength(0)
    expect(grouped.default.path).toBe(DEFAULT_PROJECT_PATH)
  })

  it('keeps the runtime ordering: first project owns the newest thread', () => {
    const older = row({ thread_id: 'older' as ThreadSummary['thread_id'], workspace: '/w/old' })
    const newer = row({ thread_id: 'newer' as ThreadSummary['thread_id'], workspace: '/w/new' })
    const grouped = groupThreadsByProject([newer, older])
    expect(grouped.projects.map((project) => project.path)).toEqual(['/w/new', '/w/old'])
  })

  it('does not cross-contaminate same-named projects in different folders', () => {
    const grouped = groupThreadsByProject([
      row({ workspace: '/code/ark' }),
      row({ workspace: '/work/ark' })
    ])
    expect(grouped.projects).toHaveLength(2)
    expect(grouped.projects.every((project) => project.label === 'ark')).toBe(true)
    expect(grouped.projects.map((project) => project.parent).sort()).toEqual(['code', 'work'])
  })

  it('inherits the active workspace for threads without an override', () => {
    const grouped = groupThreadsByProject(
      [row({ thread_id: 'a' as ThreadSummary['thread_id'] }), row({ workspace: '/w/qdc' })],
      '/code/ark'
    )
    expect(grouped.default.threads).toHaveLength(0)
    const ark = grouped.projects.find((project) => project.path === '/code/ark')
    expect(ark?.threads.map((thread) => thread.thread_id)).toEqual(['a'])
    expect(grouped.projects.map((project) => project.path)).toEqual(['/code/ark', '/w/qdc'])
  })

  it('keeps the active group first while it still owns threads', () => {
    const grouped = groupThreadsByProject(
      [row({ workspace: '/w/old' }), row({})],
      '/code/ark'
    )
    expect(grouped.projects.map((project) => project.path)).toEqual([
      '/code/ark',
      '/w/old'
    ])
    expect(grouped.projects[0]?.threads).toHaveLength(1)
  })

  it('prefers the explicit override over the active workspace', () => {
    const grouped = groupThreadsByProject(
      [row({ workspace: '/w/override' })],
      '/code/ark'
    )
    // The override owns the thread; the active root holds no thread here, so
    // its group is dropped instead of shell-ing the rail.
    expect(grouped.projects.map((project) => project.path)).toEqual([
      '/w/override'
    ])
    expect(grouped.projects[0]?.threads).toHaveLength(1)
    expect(grouped.default.threads).toHaveLength(0)
  })

  it('drops the active group once its last thread is deleted (bulk delete clears the project)', () => {
    // The rail's bulk delete folds rows out of the store; with every thread of
    // the active project gone the project must vanish with them, not linger
    // as an unremovable empty group.
    const grouped = groupThreadsByProject(
      [row({ workspace: '/w/old' })],
      '/code/ark'
    )
    expect(grouped.projects.map((project) => project.path)).toEqual(['/w/old'])
    expect(grouped.projects.some((project) => project.path === '/code/ark')).toBe(false)
  })

  it('keeps the runtime ordering untouched when no active workspace is given', () => {
    const grouped = groupThreadsByProject([row({ workspace: '/w/old' }), row({ workspace: '/w/new' })])
    expect(grouped.projects.map((project) => project.path)).toEqual(['/w/old', '/w/new'])
  })
})

describe('projectMatchesNeedle', () => {
  const project = { path: '/code/ark', label: 'ark', parent: 'code', threads: [] }

  it('matches label or path, case-insensitively', () => {
    expect(projectMatchesNeedle(project, 'ARK')).toBe(true)
    expect(projectMatchesNeedle(project, '/code')).toBe(true)
    expect(projectMatchesNeedle(project, 'qdc')).toBe(false)
  })

  it('matches everything on an empty needle', () => {
    expect(projectMatchesNeedle(project, '')).toBe(true)
  })
})
