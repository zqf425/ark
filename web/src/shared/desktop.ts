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

/**
 * Persist bytes at an absolute path (typically `pickFolder()` output plus a
 * file name) via the desktop shell's `save_file` command. Throws when the
 * shell is absent so callers can fall back to a browser download instead.
 */
export async function saveFile(path: string, bytes: Uint8Array): Promise<string> {
  const bridge = internals()
  if (!bridge) throw new Error('save_file requires the ark desktop shell')
  return bridge.invoke<string>('save_file', { path, bytes: Array.from(bytes) })
}

/**
 * System-level notification (Windows toast / macOS banner) posted by the
 * desktop shell's `notify_session_event` command. Resolves `false` when the
 * shell is absent (plain browser), the OS blocked the notification, or the
 * delivery failed — callers must keep their in-page surface in that case.
 * Clicking the notification does nothing: the payload is display text only.
 */
export async function postSystemNotification(title: string, body: string): Promise<boolean> {
  const bridge = internals()
  if (!bridge) return false
  try {
    return await bridge.invoke<boolean>('notify_session_event', { title, body })
  } catch {
    return false
  }
}
