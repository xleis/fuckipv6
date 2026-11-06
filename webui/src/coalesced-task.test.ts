import { afterEach, describe, expect, it, vi } from "vitest";
import { createCoalescedTask } from "./coalesced-task";

afterEach(() => {
  vi.useRealTimers();
});

describe("createCoalescedTask", () => {
  it("turns a 500-event burst into one bounded run", () => {
    vi.useFakeTimers();
    const run = vi.fn();
    const task = createCoalescedTask(run, 150);

    for (let index = 0; index < 500; index += 1) task.schedule();

    expect(run).not.toHaveBeenCalled();
    vi.advanceTimersByTime(149);
    expect(run).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(run).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(150);
    expect(run).toHaveBeenCalledTimes(1);
    task.dispose();
  });

  it("clears the pending follow-up when disposed", () => {
    vi.useFakeTimers();
    const run = vi.fn();
    const task = createCoalescedTask(run);

    task.schedule();
    task.schedule();
    task.dispose();
    vi.runAllTimers();

    expect(run).not.toHaveBeenCalled();
  });
});
