import { useEffect, useState } from "react";

const COPIED_MS = 2_000;

export type CopyState = "idle" | "done" | "failed";

/** Copies text to the clipboard; the result shows for two seconds, then reads idle again. */
export function useCopy(): [CopyState, (text: string) => void] {
  const [state, setState] = useState<CopyState>("idle");
  useEffect(() => {
    if (state === "idle") return;
    const timer = setTimeout(() => setState("idle"), COPIED_MS);
    return () => clearTimeout(timer);
  }, [state]);
  const copy = (text: string) =>
    void Promise.resolve()
      .then(() => navigator.clipboard.writeText(text))
      .then(
        () => setState("done"),
        () => setState("failed"),
      );
  return [state, copy];
}
