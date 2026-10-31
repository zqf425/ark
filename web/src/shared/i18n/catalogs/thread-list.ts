/**
 * Code-session thread rail: list header, scope filter, row actions, empty states.
 *
 * Row actions live on the thread list rather than a context menu, so every
 * verb here is a real button label that must follow the display language.
 */
export const THREAD_LIST_MESSAGES = {
  'thread.list.title': ['会话', 'Threads'],
  'thread.list.create': ['新会话', 'New thread'],
  'thread.list.creating': ['创建中…', 'Creating…'],
  'thread.list.search': ['搜索已加载会话', 'Search loaded threads'],
  'thread.list.scopeAria': ['会话范围', 'Thread scope'],
  'thread.list.scopeRecent': ['最近', 'Recent'],
  'thread.list.scopeArchived': ['归档', 'Archived'],
  'thread.list.loading': ['载入线程列表…', 'Loading threads…'],
  'thread.list.emptyTitle': ['还没有会话', 'No threads yet'],
  'thread.list.emptyHint': ['点击“新建”开始。', 'Select "New thread" to start.'],
  'thread.list.noMatchTitle': ['没有匹配的会话', 'No matching threads'],
  'thread.list.noMatchQueryHint': ['尝试更换搜索词，搜索仅针对本地已加载列表。', 'Try another term; search only covers the loaded list.'],
  'thread.list.noMatchScopeHint': ['当前范围内没有会话。', 'No threads in this scope.'],
  'thread.list.boundary': ['线程列表渲染失败', 'Thread list failed to render'],
  'thread.list.refresh': ['刷新列表', 'Refresh list'],
  'thread.list.count': ['显示 {visible}/{total}', 'Showing {visible}/{total}'],

  'thread.row.untitled': ['未命名会话', 'Untitled thread'],
  'thread.row.archived': ['已归档', 'Archived'],
  'thread.row.rename': ['重命名', 'Rename'],
  'thread.row.fork': ['分叉', 'Fork'],
  'thread.row.restore': ['恢复', 'Restore'],
  'thread.row.archive': ['归档', 'Archive'],
  'thread.row.delete': ['删除', 'Delete'],
  'thread.row.deleteConfirm': [
    '删除会话“{title}”？此操作不可恢复。',
    'Delete the thread "{title}"? This cannot be undone.'
  ],

  'thread.canvas.boundaryRow': ['该条消息渲染失败', 'This message failed to render'],
  'thread.canvas.boundaryRewind': ['回退入口渲染失败', 'The rewind control failed to render']
} as const
