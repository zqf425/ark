/**
 * 待由 ark-contract 生成，CI 校验 diff 为空。
 * Placeholder for the code-generated id newtypes (design §6.3).
 *
 * The Rust side wraps these in `ThreadId` / `TurnId` / `ItemId` newtypes that
 * serialize as UUID strings. Until `ark-contract` generation lands, the
 * renderer treats them as opaque strings and never parses them.
 */
export type ThreadId = string
export type TurnId = string
export type ItemId = string
export type ChildId = string
