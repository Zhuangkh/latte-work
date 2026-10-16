import { useState } from "react";

// Presentation choices only, bounded to 64 task scopes for this app run.
// Store only explicit clicks; rendering and incoming task events never write it.
const choices = new Map<string, Record<string, boolean>>();
export function useTaskDisclosure(
  scope: string | undefined,
  name: string,
  initial = true,
) {
  const read = () =>
    scope ? (choices.get(scope)?.[name] ?? initial) : initial;
  const [state, setState] = useState(() => ({ scope, name, open: read() }));
  const open =
    state.scope === scope && state.name === name ? state.open : read();
  const toggle = () => {
    const next = !open;
    setState({ scope, name, open: next });
    if (scope) {
      const saved = { ...choices.get(scope), [name]: next };
      choices.delete(scope);
      choices.set(scope, saved);
      if (choices.size > 64) choices.delete(choices.keys().next().value!);
    }
  };
  return [open, toggle] as const;
}
