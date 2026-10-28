/** A response may update the view only when it belongs to the newest probe. */
export function isLatestWorkspaceProbe(latest: number, candidate: number): boolean {
  return latest === candidate
}

/** Avoid workspace I/O until a Code session needs the Changes projection. */
export function shouldProbeWorkspace(threadId: string | null, workspacePath: string): boolean {
  return threadId !== null && workspacePath.length > 0
}
