/**
 * Memory surface (`/v1/memory`).
 *
 * The runtime already owns the store and the injection policy (docs/modules/
 * ark-store.md); this API layer only maps HTTP shapes.
 */
import type {
  CreateMemoryRequest,
  MemoryDiagnosticsResponse,
  MemoryEntry,
  MemoryListResponse,
  UpdateMemoryRequest
} from '../../../shared/contracts'
import { del, get, patch, post } from '../../../shared/api/client'

/** `GET /v1/memory`. */
export function listMemories(signal?: AbortSignal): Promise<MemoryListResponse> {
  return get<MemoryListResponse>('/v1/memory', signal ? { signal } : {})
}

/** `POST /v1/memory` — 201 with the created entry. */
export function createMemory(request: CreateMemoryRequest): Promise<MemoryEntry> {
  return post<MemoryEntry>('/v1/memory', request)
}

/** `PATCH /v1/memory/{id}` — optional-field patch. */
export function updateMemory(memoryId: string, request: UpdateMemoryRequest): Promise<MemoryEntry> {
  return patch<MemoryEntry>(`/v1/memory/${memoryId}`, request)
}

/** `DELETE /v1/memory/{id}` — deleting is how an entry stops being injected. */
export function deleteMemory(memoryId: string): Promise<void> {
  return del<void>(`/v1/memory/${memoryId}`)
}

/** `GET /v1/memory/diagnostics`. */
export function getMemoryDiagnostics(signal?: AbortSignal): Promise<MemoryDiagnosticsResponse> {
  return get<MemoryDiagnosticsResponse>('/v1/memory/diagnostics', signal ? { signal } : {})
}
