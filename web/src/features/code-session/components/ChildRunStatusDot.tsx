/**
 * Status dot for one child run, in the shape mainstream agent UIs use: a small
 * colored dot that spins (queued), pulses (running) or sits still colored by
 * the outcome. One component because three CSS states must stay in lockstep
 * with the lifecycle vocabulary of `childCompletionStatusKey`.
 */
export function ChildRunStatusDot({ state }: { state: 'queued' | 'running' | 'completed' | 'failed' | 'aborted' | 'ended' }): JSX.Element {
  const modifier =
    state === 'running'
      ? 'is-running'
      : state === 'queued'
        ? 'is-queued'
        : state === 'completed'
          ? 'is-ok'
          : state === 'failed' || state === 'aborted'
            ? 'is-error'
            : 'is-ended'
  return <span className={`ark-childdot ${modifier}`} aria-hidden="true" />
}
