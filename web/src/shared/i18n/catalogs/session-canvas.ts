/**
 * Session canvas: header, empty states, message bubbles, tool cards, gates,
 * stream footer and the composer.
 *
 * Server-authored prose (`item.text`, `approval.summary`, `request.prompt`, a
 * runtime `error` message) is rendered verbatim and never looked up here: the
 * runtime decides that language, the client only owns its own labels.
 */
export const SESSION_CANVAS_MESSAGES = {
  'canvas.empty.title': ['开始一个会话', 'Start a session'],
  'canvas.empty.hint': ['新建或选择一个会话，再描述希望 ark 完成的工作。', 'Create or select a thread, then describe what ark should do.'],
  'canvas.empty.create': ['新建会话', 'New thread'],
  'canvas.untitled': ['未命名会话', 'Untitled thread'],
  'canvas.live': ['正在执行', 'Running'],
  'canvas.turnCount': ['{count} 个回合', '{count} turns'],
  'canvas.ready': ['准备开始', 'Ready'],
  'canvas.detailLoading': ['正在载入会话…', 'Loading the thread…'],
  'canvas.detailFailed': ['载入失败：{error}', 'Failed to load: {error}'],
  'canvas.emptyThreadTitle': ['会话为空', 'Empty thread'],
  'canvas.emptyThreadHint': ['在下方输入框发送第一条消息。', 'Send the first message from the composer below.'],
  'canvas.jumpToLatest': ['跳到最新', 'Jump to latest'],

  'canvas.author.you': ['你', 'You'],
  'canvas.reasoning': ['思考过程', 'Reasoning'],
  'canvas.aborted': ['已中断', 'Interrupted'],
  'canvas.failed': ['失败', 'Failed'],
  'canvas.generating': ['正在生成', 'Generating'],

  'canvas.tool.running': ['运行中', 'Running'],
  'canvas.tool.completed': ['已完成', 'Completed'],
  'canvas.tool.succeeded': ['成功', 'Succeeded'],
  'canvas.tool.failed': ['失败', 'Failed'],
  'canvas.tool.arguments': ['参数', 'Arguments'],
  'canvas.tool.output': ['输出', 'Output'],
  'canvas.toolResult.name': ['工具结果', 'tool result'],

  'canvas.approval.title': ['需要审批', 'Approval required'],
  'canvas.approval.approved': ['已批准', 'Approved'],
  'canvas.approval.rejected': ['已拒绝', 'Rejected'],
  'canvas.approval.approve': ['批准', 'Approve'],
  'canvas.approval.reject': ['拒绝', 'Reject'],
  'canvas.approval.failed': ['审批提交失败', 'Failed to submit the approval'],

  'canvas.userInput.title': ['需要你的输入', 'Input required'],
  'canvas.userInput.answered': ['已回答', 'Answered'],
  'canvas.userInput.placeholder': ['输入回复后提交', 'Type a reply, then submit'],
  'canvas.userInput.failed': ['回复提交失败', 'Failed to submit the reply'],

  'canvas.child.noSummary': ['子代理未提供摘要。', 'The subagent reported no summary.'],
  'canvas.child.status.queued': ['排队中', 'Queued'],
  'canvas.child.status.running': ['执行中', 'Running'],
  'canvas.child.status.completed': ['已完成', 'Completed'],
  'canvas.child.status.failed': ['失败', 'Failed'],
  'canvas.child.status.aborted': ['已中止', 'Aborted'],
  'canvas.child.status.ended': ['已结束', 'Finished'],

  'canvas.composer.steerLead': ['补充给正在执行的会话', 'Add to the running turn'],
  'canvas.composer.describeLead': ['描述本次会话要完成的工作', 'Describe the work for this thread'],
  'canvas.composer.keys': ['发送', 'to send'],
  'canvas.composer.newline': ['换行', 'for a new line'],
  'canvas.composer.steerPlaceholder': ['向运行中的回合补充指令（Enter steer）', 'Add instructions to the running turn (Enter to steer)'],
  'canvas.composer.sendPlaceholder': ['发送消息（Enter 发送，Shift+Enter 换行）', 'Send a message (Enter sends, Shift+Enter adds a line)'],
  'canvas.composer.noThreadPlaceholder': ['先新建或选择一个会话', 'Create or select a thread first'],
  'canvas.composer.steerAria': ['向运行中的回合补充指令', 'Add instructions to the running turn'],
  'canvas.composer.sendAria': ['发送消息', 'Send a message'],
  'canvas.composer.streamDown': ['事件流未连接', 'Event stream disconnected'],
  'canvas.composer.streamUp': ['事件流已连接', 'Event stream connected'],
  'canvas.composer.interrupt': ['中断', 'Interrupt'],
  'canvas.composer.steer': ['Steer', 'Steer'],
  'canvas.composer.send': ['发送', 'Send'],

  'canvas.footer.connecting': ['连接中', 'Connecting'],
  'canvas.footer.open': ['已连接', 'Connected'],
  'canvas.footer.reconnecting': ['重连中', 'Reconnecting'],
  'canvas.footer.closed': ['未连接', 'Disconnected'],
  'canvas.footer.generating': ['生成中', 'Generating'],

  'canvas.relative.justNow': ['刚刚', 'just now'],
  'canvas.relative.minutes': ['{count} 分钟前', '{count}m ago'],
  'canvas.relative.hours': ['{count} 小时前', '{count}h ago'],
  'canvas.relative.days': ['{count} 天前', '{count}d ago'],
  'canvas.retryAttempt': ['第 {attempt} 次重试：{message}', 'Retry {attempt}: {message}'],

  'canvas.workspace.missing': ['工作区路径不存在', 'The workspace path does not exist'],
  'canvas.workspace.notGit': ['不是 Git 仓库', 'Not a git repository'],
  'canvas.workspace.unknown': ['工作区状态未知', 'Workspace state unknown'],
  'canvas.workspace.dirtyCount': ['{count} 项未提交改动', '{count} uncommitted changes'],
  'canvas.workspace.dirty': ['有未提交改动', 'Uncommitted changes'],
  'canvas.workspace.clean': ['工作区干净', 'Workspace clean'],

  'canvas.child.meta.model': ['{model}', '{model}'],
  'canvas.child.meta.toolInvocations': ['{count} 次工具调用', '{count} tool calls'],
  'canvas.child.meta.resumed': ['已恢复 {count} 次', 'Resumed {count} times'],

  'canvas.childRuns.title': ['子代理', 'Subagents'],
  'canvas.childRuns.active': ['{count} 活跃', '{count} active'],
  'canvas.childRuns.abort': ['终止', 'Abort'],
  'canvas.childRuns.aborting': ['终止中…', 'Aborting…'],
  'canvas.childRuns.resume': ['恢复', 'Resume'],
  'canvas.childRuns.steer': ['补充指令', 'Add instructions'],
  'canvas.childRuns.steerPrompt': ['补充给子代理的指令', 'Instructions to add to the subagent'],
  'canvas.childRuns.loadFailed': ['读取子代理失败', 'Failed to read subagents'],
  'canvas.childRuns.actionFailed': ['子代理操作失败', 'The subagent action failed'],
  'canvas.childRuns.completedToast': ['子代理「{label}」{status}', 'Subagent "{label}" {status}']
} as const
