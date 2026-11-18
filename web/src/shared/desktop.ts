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
 * Hand a web link to the OS default browser instead of letting the renderer
 * navigate its own document.
 *
 * The renderer is a single-page app without a router or history UI, and the
 * desktop window is the only way back to the workspace: an in-place navigation
 * to a foreign document replaces the app with a page nothing can leave. The
 * shell therefore refuses such navigations and this helper is how a link
 * leaves the app at all. In a plain browser the same intent is a new tab, which
 * also keeps the app document alive.
 *
 * Only `http(s)` targets are handed out: `javascript:`/`data:`/`file:` links in
 * model prose are not navigation. Scheme policy is re-checked by the shell —
 * this side only resolves the reference, because prose links may be relative.
 * A failed bridge call is deliberately silent: falling back to an in-webview
 * navigation would reintroduce exactly the trap this path exists to prevent.
 */
export async function openExternal(href: string): Promise<void> {
  const url = webUrl(href)
  if (!url) return
  const bridge = internals()
  if (!bridge) {
    window.open(url, '_blank', 'noopener,noreferrer')
    return
  }
  try {
    await bridge.invoke<null>('open_external', { url })
  } catch {
    // The shell owns the policy and its failure mode; swallow here.
  }
}

/** Absolute `http(s)` form of a prose link, or null when it is not one. */
function webUrl(href: string): string | null {
  try {
    const url = new URL(href, window.location.href)
    return url.protocol === 'http:' || url.protocol === 'https:' ? url.toString() : null
  } catch {
    return null
  }
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
