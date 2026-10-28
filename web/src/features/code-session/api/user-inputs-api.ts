/**
 * Structured user-input gate. `POST /v1/user-inputs/{id}` with `{ answer }`.
 */
import { post } from '../../../shared/api/client'
import type { ResolveUserInputRequest } from '../../../shared/contracts'

export function answerUserInput(requestId: string, answer: string): Promise<void> {
  const body: ResolveUserInputRequest = { answer }
  return post<void>(`/v1/user-inputs/${requestId}`, body)
}
