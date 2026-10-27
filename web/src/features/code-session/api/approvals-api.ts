/**
 * Approval gate surface. `POST /v1/approvals/{id}` with `{ approved }`.
 */
import { post } from '../../../shared/api/client'

export function resolveApproval(approvalId: string, approved: boolean): Promise<void> {
  return post<void>(`/v1/approvals/${approvalId}`, { approved })
}
