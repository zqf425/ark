/** Maps terminal child-run states to user-facing Chinese labels. */
export function childCompletionStatus(status: string): string {
  switch (status) {
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
