export type SnapshotPatch =
  | { kind: "replace"; value: unknown }
  | {
      kind: "object";
      set: Array<[string, SnapshotPatch]>;
      remove: string[];
    }
  | {
      kind: "array";
      length: number;
      set: Array<[number, SnapshotPatch]>;
    };

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Produce a sparse immutable update. Large snapshots that only change one
 * member therefore do not cross the worker boundary in full. */
export function diffSnapshot(
  previous: unknown,
  next: unknown,
): SnapshotPatch | null {
  if (Object.is(previous, next)) return null;

  if (Array.isArray(previous) && Array.isArray(next)) {
    const set: Array<[number, SnapshotPatch]> = [];
    const sharedLength = Math.min(previous.length, next.length);
    for (let index = 0; index < sharedLength; index += 1) {
      const child = diffSnapshot(previous[index], next[index]);
      if (child) set.push([index, child]);
    }
    for (let index = sharedLength; index < next.length; index += 1) {
      set.push([index, { kind: "replace", value: next[index] }]);
    }
    if (set.length === 0 && previous.length === next.length) return null;
    return { kind: "array", length: next.length, set };
  }

  if (record(previous) && record(next)) {
    const set: Array<[string, SnapshotPatch]> = [];
    const remove: string[] = [];
    for (const key of Object.keys(next)) {
      if (!Object.prototype.hasOwnProperty.call(previous, key)) {
        set.push([key, { kind: "replace", value: next[key] }]);
        continue;
      }
      const child = diffSnapshot(previous[key], next[key]);
      if (child) set.push([key, child]);
    }
    for (const key of Object.keys(previous)) {
      if (!Object.prototype.hasOwnProperty.call(next, key)) remove.push(key);
    }
    if (set.length === 0 && remove.length === 0) return null;
    return { kind: "object", set, remove };
  }

  return { kind: "replace", value: next };
}

export function applySnapshotPatch<T>(previous: T, patch: SnapshotPatch): T {
  return apply(previous, patch) as T;
}

function apply(previous: unknown, patch: SnapshotPatch): unknown {
  if (patch.kind === "replace") return patch.value;

  if (patch.kind === "object") {
    if (!record(previous)) return previous;
    const next: Record<string, unknown> = { ...previous };
    for (const key of patch.remove) delete next[key];
    for (const [key, child] of patch.set) {
      next[key] = apply(previous[key], child);
    }
    return next;
  }

  if (!Array.isArray(previous)) return previous;
  const next = previous.slice(0, patch.length);
  for (const [index, child] of patch.set) {
    next[index] = apply(previous[index], child);
  }
  return next;
}
