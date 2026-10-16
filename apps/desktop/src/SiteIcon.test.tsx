// @vitest-environment jsdom
import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { SiteIcon, rememberSiteIcon } from "./SiteIcon";
afterEach(cleanup);
it("replaces a failed conventional icon with the native page's declared icon", () => {
  const url = "https://custom-icon.example/doc";
  const view = render(<SiteIcon url={url} />);
  fireEvent.error(view.container.querySelector("img")!);
  expect(view.container.querySelector("img")).toBeNull();
  const icon = "data:image/png;base64,aWNvbg==";
  act(() => rememberSiteIcon(url, icon));
  const image = view.container.querySelector("img")!;
  expect(image.getAttribute("src")).toBe(icon);
  fireEvent.load(image);
  expect(view.container.querySelector("svg")).toBeNull();
  view.rerender(<SiteIcon url="https://other-icon.example/doc" />);
  expect(view.container.querySelector("img")?.getAttribute("src")).toBe(
    "https://other-icon.example/favicon.ico",
  );
  expect(view.container.querySelector("svg")).toBeTruthy();
});
it("does not place arbitrary URLs or oversized data into the icon cache", () => {
  const url = "https://safe-icon.example/";
  rememberSiteIcon(url, "javascript:alert(1)");
  rememberSiteIcon(url, "data:image/png;base64," + "A".repeat(100 * 1024));
  const view = render(<SiteIcon url={url} />);
  expect(view.container.querySelector("img")?.getAttribute("src")).toBe(
    "https://safe-icon.example/favicon.ico",
  );
});
