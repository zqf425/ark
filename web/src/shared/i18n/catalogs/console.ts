/**
 * ARK Console shell (approved proposal A): top bar, activity rail, status bar,
 * command palette, new-task page and the full-screen changes review.
 *
 * Panel/tab ids stay English literals; only visible strings are translated.
 * Every key has a zh-CN and en-US column; the en-US column must not contain
 * CJK, and identical values are only legitimate for ASCII identifiers.
 */
export const CONSOLE_MESSAGES = {
  /* global top bar */
  'console.top.workspace': ['工作区', 'Workspace'],
  'console.top.branch': ['分支', 'Branch'],
  'console.top.search': ['搜索会话、文件、运行命令…', 'Search threads, files, commands…'],
  'console.top.running': ['运行中', 'Running'],
  'console.top.idle': ['就绪', 'Ready'],
  'console.top.theme': ['切换主题', 'Toggle theme'],
  'console.top.notifications': ['通知', 'Notifications'],
  'console.top.openCommand': ['打开命令面板', 'Open command palette'],
  'console.top.settings': ['设置', 'Settings'],
  'console.top.backToWorkbench': ['返回工作台', 'Back to workbench'],
  'console.top.refresh': ['重新检查运行时', 'Recheck runtime'],

  /* activity rail */

  /* status bar */
  'console.status.connected': ['arkd 已连接', 'arkd connected'],
  'console.status.offline': ['arkd 未连接', 'arkd offline'],
  'console.status.checking': ['检查中', 'Checking'],
  'console.status.changes': ['{count} 个变更', '{count} changes'],
  'console.status.changesClean': ['工作区干净', 'No local changes'],
  'console.status.commandHint': ['命令面板', 'Command palette'],

  /* inspector */
  'console.inspector.title': ['检查器', 'Inspector'],
  'console.inspector.collapse': ['折叠检查器', 'Collapse inspector'],
  'console.inspector.expand': ['展开检查器', 'Expand inspector'],

  /* command palette */
  'console.palette.title': ['命令面板', 'Command palette'],
  'console.palette.placeholder': ['输入命令、会话或文件…', 'Type a command, thread or file…'],
  'console.palette.groupActions': ['操作', 'Actions'],
  'console.palette.groupThreads': ['会话', 'Threads'],
  'console.palette.actionNew': ['新建任务', 'New task'],
  'console.palette.actionReview': ['审查工作区变更', 'Review workspace changes'],
  'console.palette.actionWorkbench': ['返回工作台', 'Back to workbench'],
  'console.palette.actionInspector': ['切换检查器', 'Toggle inspector'],
  'console.palette.actionSettings': ['打开设置', 'Open settings'],
  'console.palette.empty': ['没有匹配结果', 'No matching results'],
  'console.palette.hintNavigate': ['选择', 'Navigate'],
  'console.palette.hintOpen': ['打开', 'Open'],
  'console.palette.hintClose': ['关闭', 'Close'],

  /* new task page */
  'console.new.kicker': ['Code 运行时就绪', 'Code runtime ready'],
  'console.new.title': ['从这里开始一个新任务', 'Start a new task'],
  'console.new.subtitle': [
    '描述目标，ark 会自己读代码、改文件、跑测试。你可以随时接管。',
    'Describe the goal; ark reads code, edits files and runs tests. You can take over anytime.'
  ],
  'console.new.failed': ['创建任务失败', 'Could not create the task'],
  'console.new.recentHint': ['发送后进入会话画布，可随时接管。', 'Sending opens the session canvas; you can take over anytime.'],

  /* changes review */
  'console.review.title': ['变更审查', 'Review changes'],
  'console.review.files': ['变更集', 'Changed files'],
  'console.review.filter': ['过滤文件…', 'Filter files…'],
  'console.review.split': ['并排', 'Side by side'],
  'console.review.unified': ['统一', 'Unified'],
  'console.review.original': ['原始 · HEAD', 'Original · HEAD'],
  'console.review.modified': ['修改后 · 工作区', 'Modified · workspace'],
  'console.review.notes': ['审查意见', 'Review notes'],
  'console.review.notesEmpty': ['暂无审查意见。', 'No review notes yet.'],
  'console.review.notesHint': ['在工作台发起评审回合后，子代理建议会显示在这里。', 'Start a review turn in the workbench to collect subagent findings here.'],
  'console.review.noSelection': ['选择一个文件查看差异', 'Select a file to view its diff'],
  'console.review.noChanges': ['当前工作区没有未提交变更。', 'There are no uncommitted changes.'],
  'console.review.loading': ['正在读取工作区差异…', 'Loading workspace diff…'],
  'console.review.acceptAll': ['全部接受', 'Accept all'],
  'console.review.rewind': ['回退本回合', 'Rewind turn'],
  'console.review.commit': ['提交变更', 'Commit changes'],
  'console.review.addComment': ['添加行内评论', 'Add inline comment'],
  'console.review.staged': ['已暂存', 'Staged'],
  'console.review.unstaged': ['未暂存', 'Unstaged'],
  'console.review.untracked': ['未跟踪', 'Untracked'],
  'console.review.noPatch': ['该文件没有可用的行级补丁。', 'No line-level patch for this file.'],
  'console.review.binary': ['二进制文件', 'Binary file'],
  'console.review.truncated': ['差异已截断', 'Diff truncated'],
  'console.review.openWorkbench': ['返回工作台', 'Back to workbench']
} as const
