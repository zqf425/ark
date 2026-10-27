/**
 * Structured user-input gate. `POST /v1/user-inputs/{id}` with `{ answer }`.
 */
import { post } from '../../../shared/api/client'

export function answerUserInput(requestId: string, answer: string): Promise<void> {
  return post<void>(`/v1/user-inputs/${requestId}`, { answer })
}
