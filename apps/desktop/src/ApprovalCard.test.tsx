// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { ApprovalCard } from "./ApprovalCard";
afterEach(cleanup);
const request = {
  kind: "approval" as const,
  request_id: "r",
  tool: "Write",
  input: { file_path: "approved.txt", content: "approved" },
};
it("shows one readable operation with explicit one-time decisions and collapsed raw parameters", () => {
  const decide = vi.fn();
  render(<ApprovalCard request={request} onDecision={decide} />);
  expect(screen.getAllByText("是否允许写入 approved.txt？")).toHaveLength(1);
  expect(screen.getByText("approved")).toBeTruthy();
  expect(screen.queryByText("未收到工具结果。")).toBeNull();
  expect(screen.getByText("操作参数").closest("details")?.open).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "拒绝" }));
  fireEvent.click(screen.getByRole("button", { name: "允许一次" }));
  expect(decide.mock.calls).toEqual([[false], [true]]);
});
it("disables both decisions during submission or disconnection", () => {
  const decide = vi.fn();
  render(<ApprovalCard request={request} disabled busy onDecision={decide} />);
  for (const button of screen.getAllByRole("button")) fireEvent.click(button);
  expect(decide).not.toHaveBeenCalled();
});
it("renders pending history as a collapsed record without actionable buttons", () => {
  render(
    <ApprovalCard
      request={request}
      presentation="record"
      onDecision={vi.fn()}
    />,
  );
  expect(screen.getByText("等待审批").closest("details")?.open).toBe(false);
  expect(screen.queryByRole("button")).toBeNull();
  expect(screen.queryByText("审批已过期")).toBeNull();
});
it("collapses resolved approvals without hiding an execution failure behind allowed status", () => {
  render(
    <ApprovalCard
      request={request}
      resolved
      decision="allowed"
      tool={{
        key: 1,
        type: "tool",
        name: "Write",
        status: "failed",
        output: "permission denied",
      }}
      onDecision={vi.fn()}
    />,
  );
  expect(screen.getByText("执行失败").closest("details")?.open).toBe(false);
  expect(screen.getByText("permission denied")).toBeTruthy();
  expect(screen.queryByRole("button")).toBeNull();
});
it("does not offer an empty allow response for an old-server user question", () => {
  const decide = vi.fn();
  render(
    <ApprovalCard
      request={{ ...request, tool: "AskUserQuestion" }}
      onDecision={decide}
    />,
  );
  expect(screen.getByText(/当前主机服务尚不支持问题交互/)).toBeTruthy();
  expect(screen.queryByText(/是否允许/)).toBeNull();
  expect(screen.queryByRole("button", { name: "允许一次" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "跳过问题" }));
  expect(decide).toHaveBeenCalledWith(false);
});
