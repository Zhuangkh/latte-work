import type { HostedProject } from "./projectCatalog";

const storageKey = "latte-work.project-order.v1";
export const projectOrderKey = (
  project: Pick<HostedProject, "hostId" | "id">,
) => JSON.stringify([project.hostId, project.id]);

export function readProjectOrder(): string[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(storageKey) ?? "[]");
    return Array.isArray(value)
      ? [
          ...new Set(
            value.filter((key): key is string => typeof key === "string"),
          ),
        ]
      : [];
  } catch {
    return [];
  }
}
export function writeProjectOrder(order: string[]) {
  try {
    localStorage.setItem(storageKey, JSON.stringify(order));
  } catch {
    /* The in-memory layout remains usable when preferences cannot be saved. */
  }
}
export function orderProjects(projects: HostedProject[], order: string[]) {
  const remaining = new Map(projects.map((p) => [projectOrderKey(p), p]));
  const result: HostedProject[] = [];
  for (const key of order) {
    const project = remaining.get(key);
    if (project) {
      result.push(project);
      remaining.delete(key);
    }
  }
  return [...result, ...remaining.values()];
}

/** Shared insertion rule for drag previews, keyboard moves and saved ordering. */
export function moveProjectKeys(
  keys: string[],
  source: string,
  target: string,
  after: boolean,
) {
  if (source === target || !keys.includes(source) || !keys.includes(target))
    return keys;
  const next = keys.filter((key) => key !== source);
  next.splice(next.indexOf(target) + (after ? 1 : 0), 0, source);
  return next.every((key, i) => key === keys[i]) ? keys : next;
}

/** Replace visible slots, retaining preferences for hosts not currently loaded. */
export function moveProjectOrder(
  previous: string[],
  projects: HostedProject[],
  source: string,
  target: string,
  after: boolean,
): string[] {
  const keys = orderProjects(projects, previous).map(projectOrderKey);
  const next = moveProjectKeys(keys, source, target, after);
  if (next === keys) return previous;
  const visible = new Set(keys);
  const result = previous.map((key) =>
    visible.has(key) ? next.shift()! : key,
  );
  return [...result, ...next];
}
