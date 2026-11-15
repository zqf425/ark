/**
 * Code-session thread rail: list header, scope filter, row actions, empty states.
 *
 * Row actions live on the thread list rather than a context menu, so every
 * verb here is a real button label that must follow the display language.
 */
export const THREAD_LIST_MESSAGES = {
  'thread.list.title': ['会话', 'Threads'],
  'thread.list.create': ['新会话', 'New thread'],
  'thread.list.defaultTitle': ['新会话', 'New thread'],
  'thread.list.creating': ['创建中…', 'Creating…'],
  'thread.list.groupCreate': ['在项目 {title} 下新建会话', 'New session in project {title}'],
  'thread.list.groupCreateDone': ['已在“{title}”下新建会话', 'New session created in "{title}"'],
  'thread.list.groupDelete': ['删除项目 {title} 下已结束的会话', "Delete project {title}'s finished sessions"],
  'thread.list.groupDeleteConfirm': [
    '删除项目“{title}”下所有已结束的会话（{count} 条）？正在运行的会话会保留。此操作不可恢复。',
    "Delete all {count} finished session(s) of project \"{title}\"? Running sessions are kept. This cannot be undone."
  ],
  'thread.list.groupDeleteDone': ['已删除 {count} 条会话', 'Deleted {count} session(s)'],
  'thread.list.groupDeleteSkipped': ['已删除 {deleted} 条，{skipped} 条正在运行已保留', 'Deleted {deleted}; kept {skipped} running'],
  'thread.list.schedule': ['定时任务', 'Scheduled task'],
  'thread.list.scheduleHint': ['新建一个按节奏自动执行的定时任务', 'Create a task that runs a prompt on a cadence'],
  'thread.list.search': ['搜索已加载会话', 'Search loaded threads'],
  'thread.list.scopeAria': ['会话范围', 'Thread scope'],
  'thread.list.scopeRecent': ['最近', 'Recent'],
  'thread.list.scopeArchived': ['归档', 'Archived'],
  'thread.list.projectsAria': ['项目分组', 'Project groups'],
  'thread.list.defaultProject': ['默认项目', 'Default project'],
  'thread.list.defaultProjectHint': ['未设置工作空间覆盖的会话', 'Threads without a workspace override'],
  'thread.list.groupToggle': ['展开或收起项目 {title}', 'Expand or collapse project {title}'],
  'thread.list.groupCount': ['{count} 条会话', '{count} threads'],
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
  'thread.row.menu': ['会话操作', 'Thread actions'],
  'thread.row.running': ['模型正在回复…', 'The model is replying…'],
  'thread.notify.completed': ['会话「{title}」已完成回复', 'Thread "{title}" finished replying'],
  'thread.notify.failed': ['会话「{title}」回合失败', 'A turn in "{title}" failed'],
  'thread.notify.aborted': ['会话「{title}」回合已中止', 'A turn in "{title}" was aborted'],
  'thread.notify.ended': ['会话「{title}」回合已结束', 'A turn in "{title}" ended'],
  'thread.notify.os.completed': ['会话完成', 'Thread finished'],
  'thread.notify.os.failed': ['会话失败', 'Thread failed'],
  'thread.notify.os.aborted': ['会话已中止', 'Thread aborted'],
  'thread.notify.os.ended': ['会话已结束', 'Thread ended'],
  'thread.row.archived': ['已归档', 'Archived'],
  'thread.row.forked': ['分叉会话', 'Forked thread'],
  'thread.row.forkedFrom': ['分叉自 {title}', 'Forked from {title}'],
  'thread.row.rename': ['重命名', 'Rename'],
  'thread.row.fork': ['分叉', 'Fork'],
  'thread.row.restore': ['恢复', 'Restore'],
  'thread.row.archive': ['归档', 'Archive'],
  'thread.row.delete': ['删除', 'Delete'],
  'thread.row.export': ['导出', 'Export'],
  'thread.row.exportHint': ['选择文件夹，将会话打包为 zip 保存', 'Pick a folder and save the thread as a zip'],
  'thread.row.actionFailed': ['操作失败：{message}', 'Action failed: {message}'],
  'thread.row.deleteConfirm': [
    '删除会话“{title}”？此操作不可恢复。',
    'Delete the thread "{title}"? This cannot be undone.'
  ],

  'thread.canvas.boundaryRow': ['该条消息渲染失败', 'This message failed to render'],
  'thread.canvas.boundaryRewind': ['回退入口渲染失败', 'The rewind control failed to render']
} as const
