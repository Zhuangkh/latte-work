// @vitest-environment jsdom
import { beforeEach, expect, it } from "vitest";
import {
  projectOrderKey as key,
  moveProjectOrder,
  orderProjects,
  readProjectOrder,
  writeProjectOrder,
} from "./projectOrder";
const projects = [
  { hostId: "local", id: "same", name: "Local", path: "/local" },
  { hostId: "devbox", id: "same", name: "Remote", path: "/remote" },
  { hostId: "devbox", id: "other", name: "Other", path: "/other" },
];
beforeEach(() => localStorage.clear());
it("persists a mixed-host order independently of refreshed server ordering", () => {
  const saved = moveProjectOrder(
    [],
    projects,
    key(projects[2]),
    key(projects[0]),
    false,
  );
  writeProjectOrder(saved);
  expect(
    orderProjects([projects[1], projects[0], projects[2]], readProjectOrder()),
  ).toEqual([projects[2], projects[0], projects[1]]);
});
it("keeps identical project ids on different hosts distinct", () => {
  const saved = moveProjectOrder(
    [],
    projects,
    key(projects[1]),
    key(projects[0]),
    false,
  );
  expect(orderProjects(projects, saved)).toEqual([
    projects[1],
    projects[0],
    projects[2],
  ]);
});
it("keeps absent-host slots and appends newly discovered projects", () => {
  const hidden = { hostId: "hidden", id: "x", name: "Absent", path: "/x" };
  const previous = [key(projects[0]), key(hidden), key(projects[1])];
  const next = moveProjectOrder(
    previous,
    projects,
    key(projects[1]),
    key(projects[0]),
    false,
  );
  expect(next).toEqual([
    key(projects[1]),
    key(hidden),
    key(projects[0]),
    key(projects[2]),
  ]);
  expect(orderProjects([...projects, hidden], next)).toEqual([
    projects[1],
    hidden,
    projects[0],
    projects[2],
  ]);
});
it("ignores stale drag targets and does not save an unchanged position", () => {
  const previous = projects.map(key);
  expect(
    moveProjectOrder(previous, projects, key(projects[0]), "removed", false),
  ).toBe(previous);
  expect(
    moveProjectOrder(
      previous,
      projects,
      key(projects[0]),
      key(projects[1]),
      false,
    ),
  ).toBe(previous);
});
it("reads corrupt preferences defensively", () => {
  localStorage.setItem(
    "latte-work.project-order.v1",
    JSON.stringify([key(projects[0]), false, key(projects[0])]),
  );
  expect(readProjectOrder()).toEqual([key(projects[0])]);
  localStorage.setItem("latte-work.project-order.v1", "broken");
  expect(readProjectOrder()).toEqual([]);
});
