/**
 * Per-session runtime controls: runtime overrides, context compaction, rewind.
 *
 * These keys were introduced before the sweep; they are kept verbatim so the
 * existing rewind and session-control tests keep guarding the same contract.
 */
export const SESSION_MESSAGES = {
  'session.runtime.workspace': ['工作空间', 'Workspace'],
  'session.runtime.strict': ['永远询问：命令与写入需批准', 'Ask every time: approve commands and writes'],
  'session.runtime.workspaceWrite': ['仅工作区修改：命令需批准', 'Workspace edits only: approve commands'],
  'session.runtime.unattended': ['完全访问：可越出工作区，不再询问', 'Full access: outside workspace, no approvals'],

  'session.context.compact': ['压缩上下文', 'Compact context'],
  'session.context.compacted': ['上下文已压缩', 'Context compacted'],

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
  'session.rewind.done': ['已回退，会话已恢复到所选回合之前。', 'Rewound; the session now ends before the selected turn.'],

  'session.editResend.entry': ['编辑重发', 'Edit & resend'],
  'session.editResend.entryHint': ['移除此回合及其后的内容，并把这条消息放回输入框供编辑后重新发送。', 'Removes this turn and everything after it, then puts the message back into the composer for editing and resending.'],
  'session.editResend.preparing': ['准备中…', 'Preparing…'],
  'session.editResend.failed': ['编辑重发失败', 'Edit & resend failed'],
  'session.editResend.done': ['已移除该回合，消息已放回输入框，编辑后发送即可重新开始。', 'Turn removed and the message is back in the composer; edit and send to start over.'],

  'session.runtime.reasoning': ['推理强度', 'Reasoning effort'],
  'session.runtime.reasoning.unset': ['未设置', 'Unset'],
  'session.runtime.reasoning.off': ['关闭', 'Off'],
  'session.runtime.reasoning.low': ['低', 'Low'],
  'session.runtime.reasoning.medium': ['中', 'Medium'],
  'session.runtime.reasoning.high': ['高', 'High'],
  'session.runtime.visionUnsupported': [
    '当前模型未声明识图能力：图片附件不会发送给模型。若模型确支持图片，可在供应商高级选项里勾选「支持识图」。',
    'The selected model does not declare vision, so image attachments will not reach it. If it truly supports images, enable "supports vision" in the provider form.'
  ]
} as const
