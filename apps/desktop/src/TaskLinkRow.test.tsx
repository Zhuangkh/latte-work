// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { TaskLinkRow } from "./TaskLinkRow";

afterEach(cleanup);

it("loads the site's own icon without sending the conversation URL or referrer", () => {
  const { container } = render(
    <TaskLinkRow
      url="https://github.com/latte-co/latte-work?tab=readme"
      title="Latte Work"
    />,
  );
  const icon = container.querySelector("img")!;
  expect(icon.getAttribute("src")).toBe("https://github.com/favicon.ico");
  expect(icon.getAttribute("referrerpolicy")).toBe("no-referrer");
  expect(
    screen.getByRole("link", { name: "Latte Work" }).getAttribute("href"),
  ).toBe("https://github.com/latte-co/latte-work?tab=readme");
  expect(icon.classList.contains("is-ready")).toBe(false);
  fireEvent.load(icon);
  expect(icon.classList.contains("is-ready")).toBe(true);
  expect(container.querySelector("svg")).toBeNull();
});

it("keeps a quiet link fallback after failure and retries only for a different site", () => {
  const view = render(
    <TaskLinkRow url="https://one.test/a" title="one.test" />,
  );
  fireEvent.error(view.container.querySelector("img")!);
  expect(view.container.querySelector("img")).toBeNull();
  expect(view.container.querySelector(".task-info-site-icon svg")).toBeTruthy();
  view.rerender(<TaskLinkRow url="https://one.test/b" title="one.test" />);
  expect(view.container.querySelector("img")).toBeNull();
  view.rerender(<TaskLinkRow url="https://two.test/b" title="two.test" />);
  expect(view.container.querySelector("img")?.getAttribute("src")).toBe(
    "https://two.test/favicon.ico",
  );
});

it("shows one compact address when no page title is available", () => {
  const { container } = render(
    <TaskLinkRow
      url="https://github.com/latte-co/latte-work"
      title="github.com"
    />,
  );
  expect(screen.getByRole("link").textContent).toBe(
    "github.com/latte-co/latte-work",
  );
  expect(container.querySelector("small")).toBeNull();
});

it("does not attempt icon requests for unsupported or credential-bearing URLs", () => {
  for (const url of [
    "javascript:alert(1)",
    "https://user:secret@example.test/a",
    "http://ipc.localhost/a",
  ]) {
    const view = render(<TaskLinkRow url={url} title="Resource" />);
    expect(view.container.querySelector("img")).toBeNull();
    view.unmount();
  }
});
