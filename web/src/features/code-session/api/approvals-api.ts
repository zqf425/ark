/**
 * Approval gate surface. `POST /v1/approvals/{id}` with `{ approved }`.
 */
import { post } from '../../../shared/api/client'
import type { ResolveApprovalRequest } from '../../../shared/contracts'

export function resolveApproval(approvalId: string, approved: boolean): Promise<void> {
  const body: ResolveApprovalRequest = { approved }
  return post<void>(`/v1/approvals/${approvalId}`, body)
}
