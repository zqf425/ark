/**
 * Settings -> Schedules (requirements/scheduled-tasks.md §配置).
 *
 * The cadence sentences live here because they are user-facing prose built from
 * runtime-normalized values; the wire identifiers (`manual`, `unattended`,
 * `next_run_at`) keep their spelling. Autonomy labels reuse the existing
 * `session.runtime.*` keys so permissions read the same on both screens.
 */
export const SETTINGS_SCHEDULES_MESSAGES = {
  'settings.schedules.title': ['定时任务', 'Scheduled tasks'],
  'settings.schedules.desc': ['按节奏自动创建会话并执行提示词；结果留在可打开的会话里。', 'Run a prompt on a cadence; each run creates a thread you can open.'],
  'settings.schedules.descLoading': ['定时任务列表与全局开关。', 'The task list and the master switch.'],
  'settings.schedules.loadFailed': ['加载定时任务失败', 'Failed to load scheduled tasks'],

  'settings.schedules.master.label': ['启用调度（schedules.enabled）', 'Enable scheduling (schedules.enabled)'],
  'settings.schedules.master.hint': ['关闭后不再触发任何任务；列表仍可查看和编辑。', 'When off, nothing fires; the list stays readable and editable.'],
  'settings.schedules.master.saveFailed': ['保存调度开关失败', 'Failed to save the schedule switch'],
  'settings.schedules.master.succeeded': ['调度开关已保存', 'Schedule switch saved'],
  'settings.schedules.master.rejected': ['运行时拒绝了调度开关配置', 'The runtime rejected the schedule switch'],
  'settings.schedules.master.rejectedTitle': ['保存被拒绝（result: rejected）', 'Save rejected (result: rejected)'],

  'settings.schedules.emptyTitle': ['还没有定时任务', 'No scheduled tasks yet'],
  'settings.schedules.emptyHint': ['新增一个任务，设定提示词与触发节奏。', 'Add a task with a prompt and a cadence.'],
  'settings.schedules.capReached': ['任务数已达上限（{max}），请先删除部分任务。', 'The task limit is reached ({max}); delete a task first.'],

  'settings.schedules.add': ['新增任务', 'Add task'],
  'settings.schedules.createTitle': ['新增定时任务', 'New scheduled task'],
  'settings.schedules.openAll': ['查看全部定时任务', 'View all scheduled tasks'],
  'settings.schedules.editTitle': ['编辑「{title}」', 'Edit "{title}"'],
  'settings.schedules.enabledLabel': ['启用（enabled）', 'Enabled (enabled)'],
  'settings.schedules.toggleHint': ['开关切换即保存。', 'Toggling saves immediately.'],
  'settings.schedules.toggleFailed': ['切换任务启停失败', 'Failed to toggle the task'],
  'settings.schedules.toggled': ['任务「{title}」已{state}', 'Task "{title}" {state}'],
  'settings.schedules.enabledOn': ['启用', 'enabled'],
  'settings.schedules.enabledOff': ['停用', 'disabled'],

  'settings.schedules.field.title': ['标题（可选）', 'Title (optional)'],
  'settings.schedules.field.titleHint': ['留空时由提示词首行生成。', 'When blank the first prompt line is used.'],
  'settings.schedules.field.prompt': ['提示词', 'Prompt'],
  'settings.schedules.field.promptHint': ['每次运行发送的用户消息。', 'The user message sent for every run.'],
  'settings.schedules.field.cadence': ['触发节奏', 'Cadence'],
  'settings.schedules.field.everyMinutes': ['间隔（分钟）', 'Interval (minutes)'],
  'settings.schedules.field.everyMinutesHint': ['范围 {min}–{max}。', 'Range {min}–{max}.'],
  'settings.schedules.field.timeOfDay': ['时间（HH:mm，本地时区）', 'Time (HH:mm, local)'],
  'settings.schedules.field.atTime': ['执行时刻（本地时区）', 'Run at (local time)'],
  'settings.schedules.field.weekdays': ['星期（可多选）', 'Weekdays (multi-select)'],
  'settings.schedules.field.monthDays': ['每月几号（可多选）', 'Days of month (multi-select)'],
  'settings.schedules.field.autonomy': ['权限（autonomy）', 'Permissions (autonomy)'],
  'settings.schedules.field.autonomyHint': ['留空表示使用无人值守默认值。', 'Blank means the unattended default.'],
  'settings.schedules.field.autonomyDefault': ['（使用默认）', '(use default)'],
  'settings.schedules.field.model': ['供应商 / 模型（可选）', 'Provider / model (optional)'],
  'settings.schedules.field.modelHint': ['在二级菜单中先选供应商再选模型；「跟随默认」表示不覆盖。', 'Pick a provider then a model in the two-level menu; "Follow default" clears the override.'],
  'settings.schedules.field.workspace': ['工作区（可选）', 'Workspace (optional)'],
  'settings.schedules.field.workspaceHint': ['留空表示使用全局默认工作区。', 'Blank means the global default workspace.'],
  'settings.schedules.workspace.unset': ['未设置（使用全局默认工作区）', 'Not set (using the global default workspace)'],
  'settings.schedules.workspace.pick': ['选择文件夹…', 'Choose folder…'],
  'settings.schedules.workspace.browsing': ['选择中…', 'Selecting…'],
  'settings.schedules.workspace.clear': ['清除', 'Clear'],

  'settings.schedules.saveFailed': ['保存定时任务失败', 'Failed to save the scheduled task'],
  'settings.schedules.created': ['已创建定时任务「{title}」', 'Scheduled task "{title}" created'],
  'settings.schedules.updated': ['已更新定时任务「{title}」', 'Scheduled task "{title}" updated'],
  'settings.schedules.nothingChanged': ['没有需要保存的变更', 'Nothing to save'],

  'settings.schedules.deleteFailed': ['删除定时任务失败', 'Failed to delete the scheduled task'],
  'settings.schedules.deleted': ['已删除定时任务「{title}」', 'Scheduled task "{title}" deleted'],
  'settings.schedules.deleteConfirm': [
    '删除定时任务「{title}」？历史记录会一并删除，此操作不可恢复。',
    'Delete scheduled task "{title}"? Its run history is removed too, and this cannot be undone.'
  ],

  'settings.schedules.run': ['立即运行', 'Run now'],
  'settings.schedules.running': ['运行中…', 'Starting…'],
  'settings.schedules.runStarted': ['已开始运行「{title}」', 'Started "{title}"'],
  'settings.schedules.runFailed': ['立即运行失败', 'Failed to start the run'],
  'settings.schedules.runConflict': [
    '该任务仍在运行（或调度总开关已关闭），本次未启动。',
    'The task is still running (or the master switch is off), so nothing started.'
  ],

  'settings.schedules.lastRun.label': ['最近结果', 'Last run'],
  'settings.schedules.lastRun.none': ['尚未运行', 'Never run'],
  'settings.schedules.lastRun.message': ['{status} · {time}', '{status} · {time}'],
  'settings.schedules.nextRun.label': ['下次运行', 'Next run'],
  'settings.schedules.nextRun.none': ['不调度', 'Not scheduled'],
  'settings.schedules.openThread': ['打开会话', 'Open thread'],
  'settings.schedules.threadMissing': ['本次运行没有留下会话', 'This run left no thread'],

  'settings.schedules.status.running': ['运行中', 'Running'],
  'settings.schedules.status.completed': ['成功', 'Completed'],
  'settings.schedules.status.failed': ['失败', 'Failed'],
  'settings.schedules.status.aborted': ['已中止', 'Aborted'],
  'settings.schedules.status.skipped': ['已跳过', 'Skipped'],

  'settings.schedules.kind.manual': ['仅手动', 'Manual only'],
  'settings.schedules.kind.at': ['一次性', 'One-shot'],
  'settings.schedules.kind.interval': ['按间隔', 'Interval'],
  'settings.schedules.kind.daily': ['每天', 'Daily'],
  'settings.schedules.kind.weekly': ['每周', 'Weekly'],
  'settings.schedules.kind.monthly': ['每月', 'Monthly'],

  'settings.schedules.cadence.manual': ['仅手动运行', 'Manual runs only'],
  'settings.schedules.cadence.at': ['一次性 {time}', 'Once at {time}'],
  'settings.schedules.cadence.atNone': ['一次性（未设置时刻）', 'One-shot (no time set)'],
  'settings.schedules.cadence.interval': ['每 {minutes} 分钟', 'Every {minutes} minutes'],
  'settings.schedules.cadence.daily': ['每天 {time}', 'Daily at {time}'],
  'settings.schedules.cadence.weekly': ['每{days} {time}', 'Every {days} at {time}'],
  'settings.schedules.cadence.monthly': ['每月 {days} 日 {time}', 'Monthly on day {days} at {time}'],

  'settings.schedules.weekday.0': ['周日', 'Sun'],
  'settings.schedules.weekday.1': ['周一', 'Mon'],
  'settings.schedules.weekday.2': ['周二', 'Tue'],
  'settings.schedules.weekday.3': ['周三', 'Wed'],
  'settings.schedules.weekday.4': ['周四', 'Thu'],
  'settings.schedules.weekday.5': ['周五', 'Fri'],
  'settings.schedules.weekday.6': ['周六', 'Sat'],

  'settings.schedules.error.promptRequired': ['请填写提示词。', 'A prompt is required.'],
  'settings.schedules.error.intervalRange': ['间隔必须是 {min}–{max} 之间的整数分钟。', 'The interval must be an integer between {min} and {max} minutes.'],
  'settings.schedules.error.atRequired': ['请选择一次性任务的执行时刻。', 'Choose the run time for a one-shot task.'],
  'settings.schedules.error.clockInvalid': ['时间必须是 HH:mm（24 小时制）。', 'The time must be HH:mm (24-hour).'],
  'settings.schedules.error.weekdayRequired': ['至少选择一个星期。', 'Select at least one weekday.'],
  'settings.schedules.error.monthDayRequired': ['至少选择一个月内日期。', 'Select at least one day of the month.']
} as const
