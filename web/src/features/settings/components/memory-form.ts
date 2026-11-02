/**
 * Pure memory form helpers: byte-bound validation and tag parsing.
 *
 * The runtime enforces text ≤ 16 KiB UTF-8 (`MAX_MEMORY_TEXT_BYTES`); the form
 * validates the same bound client-side so the common failure never makes a
 * round trip. Tag/source semantics (upper bounds, search) are still open in
 * docs/requirements/memory-store.md — no client-side limit is invented here.
 */
const MAX_TEXT_BYTES = 16 * 1024

export function memoryTextByteLength(text: string): number {
  return new TextEncoder().encode(text).length
}

/** `null` when acceptable, otherwise an i18n message key for the reason. */
export function memoryTextError(text: string): 'settings.memory.textRequired' | 'settings.memory.textTooLong' | null {
  if (text.trim().length === 0) return 'settings.memory.textRequired'
  if (memoryTextByteLength(text) > MAX_TEXT_BYTES) return 'settings.memory.textTooLong'
  return null
}

/** Comma-separated tags (ASCII or fullwidth comma), trimmed and deduplicated. */
export function parseMemoryTags(input: string): string[] {
  const seen = new Set<string>()
  for (const part of input.split(/[,，]/)) {
    const tag = part.trim()
    if (tag.length === 0) continue
    seen.add(tag)
  }
  return [...seen]
}
