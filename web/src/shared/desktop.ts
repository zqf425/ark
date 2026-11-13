/**
 * Minimal desktop-shell bridge (Tauri).
 *
 * The renderer also runs in plain browsers served by arkd, so every helper
 * degrades gracefully: when `__TAURI_INTERNALS__` is absent the call resolves
 * to a conservative fallback instead of touching the bridge.
 */

type TauriInternals = {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>
}

function internals(): TauriInternals | null {
  return (window as { __TAURI_INTERNALS__?: TauriInternals }).__TAURI_INTERNALS__ ?? null
}

/** True when the renderer runs inside the ark desktop shell. */
export function isDesktopShell(): boolean {
  return internals() !== null
}

/**
 * Native folder picker exposed by the desktop shell (`pick_folder` command).
 * Resolves null when the user cancels or when not running in the shell.
 */
export async function pickFolder(): Promise<string | null> {
  const bridge = internals()
  if (!bridge) return null
  return bridge.invoke<string | null>('pick_folder')
}
