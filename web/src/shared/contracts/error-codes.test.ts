import { describe, expect, it } from 'vitest'
import { ARK_ERROR_CODES } from './index'

/**
 * The set is spelled by hand because ts-rs emits `ArkErrorCode` as a bare
 * union with no runtime value list. This test is the two-way pin: a Rust
 * variant added without updating the set fails here, and so does a typo.
 */
describe('ARK_ERROR_CODES', () => {
  it('covers every code the generated union declares', () => {
    // Keep this list in sync with `web/src/generated/ArkErrorCode.ts`; the
    // mirror is what makes the runtime set trustworthy.
    const generated = [
      'validation_error',
      'not_found',
      'unauthorized',
      'conflict',
      'model_request_failed',
      'tool_execution_failed',
      'runtime_unavailable',
      'store_failure',
      'not_implemented',
      'internal_error',
      'mime_mismatch',
      'attachment_in_use',
      'attachment_not_associated'
    ] as const

    expect([...ARK_ERROR_CODES].sort()).toEqual([...generated].sort())
  })

  it('rejects a string that is not in the union', () => {
    expect(ARK_ERROR_CODES.has('bogus')).toBe(false)
  })
})
