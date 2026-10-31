/**
 * Per-session runtime controls: runtime overrides, context compaction, rewind.
 *
 * These keys were introduced before the sweep; they are kept verbatim so the
 * existing rewind and session-control tests keep guarding the same contract.
 */
export const SESSION_MESSAGES = {
  'session.runtime.title': ['本会话运行配置', 'Session runtime'],
  'session.runtime.workspace': ['工作空间', 'Workspace'],
  'session.runtime.provider': ['供应商', 'Provider'],
  'session.runtime.model': ['模型覆盖', 'Model override'],
  'session.runtime.autonomy': ['权限', 'Permissions'],
  'session.runtime.save': ['保存（下一回合生效）', 'Save for next turn'],
  'session.runtime.strict': ['永远询问：命令与写入需批准', 'Ask every time: approve commands and writes'],
  'session.runtime.workspaceWrite': ['仅工作区修改：命令需批准', 'Workspace edits only: approve commands'],
  'session.runtime.unattended': ['完全访问：可越出工作区，不再询问', 'Full access: outside workspace, no approvals'],

  'session.context.compact': ['创建上下文检查点', 'Create context checkpoint'],
  'session.context.compacted': ['上下文检查点已创建', 'Context checkpoint created'],

  'session.rewind.entry': ['回退到这里', 'Rewind here'],
  'session.rewind.confirmTitle': ['回退到此回合之前？', 'Rewind to before this turn?'],
  'session.rewind.confirmBody': [
    '该回合及其后的所有对话与事件将被移除，且不可恢复。',
    'This turn and every conversation row and event after it are removed permanently.'
  ],
  'session.rewind.confirmWorkspace': ['工作区文件不会被改动。', 'Files in the workspace are not touched.'],
  'session.rewind.busyHint': ['当前会话仍在执行，结束后才能回退。', 'This thread is still running; rewind becomes available when it finishes.'],
  'session.rewind.confirmAction': ['确认回退', 'Confirm rewind'],
  'session.rewind.cancel': ['取消', 'Cancel'],
  'session.rewind.rewinding': ['回退中…', 'Rewinding…'],
  'session.rewind.failed': ['回退失败', 'Rewind failed'],
  'session.rewind.done': ['已回退，会话已恢复到所选回合之前。', 'Rewound; the session now ends before the selected turn.']
} as const
