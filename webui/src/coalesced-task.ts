export type CoalescedTask = {
  schedule(): void;
  dispose(): void;
};

/**
 * Coalesce a synchronous event burst into one trailing run. Events that arrive
 * after the run starts open a fresh window, so a continuous stream still has
 * one bounded timer instead of an unbounded queue.
 */
export function createCoalescedTask(
  run: () => void,
  intervalMs = 150,
): CoalescedTask {
  let timer: number | null = null;
  let disposed = false;

  function schedule(): void {
    if (disposed || timer !== null) return;
    timer = window.setTimeout(() => {
      timer = null;
      if (!disposed) run();
    }, intervalMs);
  }

  return {
    schedule,
    dispose() {
      disposed = true;
      if (timer !== null) window.clearTimeout(timer);
      timer = null;
    },
  };
}
