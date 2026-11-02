/**
 * Memory settings section (the runtime memory surface).
 *
 * The runtime injects stored entries into every turn under its own budget;
 * there is deliberately no per-entry enable switch — deleting an entry is how
 * it stops being injected (docs/requirements/memory-store.md).
 */
export const SETTINGS_MEMORY_MESSAGES = {
  'settings.nav.memory.label': ['记忆', 'Memory'],
  'settings.nav.memory.detail': ['跨回合注入的长期笔记', 'Long-term notes injected across turns'],
  'settings.boundary.memory': ['记忆设置崩溃', 'Memory settings crashed'],

  'settings.memory.title': ['记忆', 'Memory'],
  'settings.memory.desc': ['保存在这里的内容会注入每一回合的上下文；删除条目即停止注入。', 'Entries stored here are injected into every turn; deleting one stops the injection.'],
  'settings.memory.loadFailed': ['读取记忆失败', 'Failed to load memories'],
  'settings.memory.storagePath': ['存储文件', 'Storage file'],
  'settings.memory.entryCount': ['条目数', 'Entries'],
  'settings.memory.lastWritten': ['最近写入', 'Last written'],
  'settings.memory.never': ['从未', 'Never'],
  'settings.memory.diagnosticsUnavailable': ['诊断信息不可用', 'Diagnostics are unavailable'],

  'settings.memory.textLabel': ['内容', 'Text'],
  'settings.memory.textPlaceholder': ['希望助手长期记住的内容', 'What the assistant should remember'],
  'settings.memory.textRequired': ['内容不能为空', 'Text must not be empty'],
  'settings.memory.textTooLong': ['内容超过 16KiB 上限', 'Text exceeds the 16 KiB bound'],
  'settings.memory.tagsLabel': ['标签（逗号分隔，可选）', 'Tags (comma separated, optional)'],
  'settings.memory.tagsPlaceholder': ['例如：偏好, 项目', 'e.g. preference, project'],
  'settings.memory.sourceLabel': ['来源（可选）', 'Source (optional)'],
  'settings.memory.create': ['新增记忆', 'Add memory'],
  'settings.memory.created': ['记忆已保存', 'Memory saved'],
  'settings.memory.createFailed': ['保存记忆失败', 'Failed to save the memory'],
  'settings.memory.updated': ['记忆已更新', 'Memory updated'],
  'settings.memory.updateFailed': ['更新记忆失败', 'Failed to update the memory'],
  'settings.memory.deleted': ['记忆已删除', 'Memory deleted'],
  'settings.memory.deleteFailed': ['删除记忆失败', 'Failed to delete the memory'],
  'settings.memory.injectHint': ['每回合按创建顺序注入，有预算上限：条目过多时较新的条目会被整条跳过。', 'Entries are injected oldest first under a per-turn budget: when they overflow, newer entries are skipped whole.'],
  'settings.memory.emptyTitle': ['暂无记忆', 'No memories'],
  'settings.memory.emptyHint': ['在上方写下希望助手长期记住的内容。', 'Write what the assistant should remember above.']
} as const
