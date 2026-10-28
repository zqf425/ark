/** Maps child-run lifecycle states to user-facing Chinese labels. */
export function childCompletionStatus(status: string): string {
  switch (status) {
    case 'queued':
      return '排队中'
    case 'running':
      return '执行中'
    case 'completed':
      return '已完成'
    case 'failed':
      return '失败'
    case 'aborted':
      return '已中止'
    default:
      return '已结束'
  }
}
