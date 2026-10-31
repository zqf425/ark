/**
 * Shared, non-feature copy: buttons, status bar, error-boundary and API fallbacks.
 *
 * Each entry is `[zh-CN, en-US]`. Keys keep the `section.meaning` shape so a
 * feature prefix always maps to exactly one catalog file.
 */
export const COMMON_MESSAGES = {
  'common.save': ['保存', 'Save'],
  'common.saving': ['保存中…', 'Saving…'],
  'common.cancel': ['取消', 'Cancel'],
  'common.delete': ['删除', 'Delete'],
  'common.remove': ['移除', 'Remove'],
  'common.retry': ['重试', 'Retry'],
  'common.submit': ['提交', 'Submit'],
  'common.submitting': ['提交中…', 'Submitting…'],
  'common.refresh': ['刷新', 'Refresh'],
  'common.refreshing': ['刷新中…', 'Refreshing…'],
  'common.edit': ['编辑', 'Edit'],
  'common.add': ['新增', 'Add'],
  'common.close': ['关闭', 'Close'],
  'common.expand': ['展开', 'Expand'],
  'common.collapse': ['收起', 'Collapse'],
  'common.failed': ['失败', 'Failed'],
  'common.success': ['成功', 'Succeeded'],
  'common.unknownError': ['未知错误', 'Unknown error'],
  'common.notSet': ['（未设置）', '(not set)'],
  'common.none': ['无', 'none'],
  'common.test': ['测试连接', 'Test connection'],
  /** Joins a short error code hint with the server's own message. */
  'common.hintSeparator': ['：', ': '],
  'common.listSeparator': ['、', ', '],

  'status.online.unknown': ['检测中', 'Checking'],
  'status.online.online': ['在线', 'Online'],
  'status.online.offline': ['离线', 'Offline'],
  'status.stream.connecting': ['连接中', 'Connecting'],
  'status.stream.open': ['已同步', 'In sync'],
  'status.stream.reconnecting': ['重连中', 'Reconnecting'],
  'status.stream.closed': ['事件流已断开', 'Event stream closed'],
  'status.refresh': ['重新检查运行时', 'Recheck runtime'],
  'status.refreshFailed': ['重新检查运行时。上次失败：{error}', 'Recheck runtime. Last failure: {error}'],
  'status.ariaSeparator': ['，', ', '],

  'ui.renderFailed': ['该区域渲染失败', 'This area failed to render'],
  'ui.toastClose': ['关闭', 'Close'],

  'api.sseBodyMissing': ['SSE 响应缺少 body', 'The SSE response has no body']
} as const
