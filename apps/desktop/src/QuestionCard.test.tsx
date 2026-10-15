// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { QuestionCard } from "./QuestionCard";
import type { Event } from "./protocol";
afterEach(cleanup);
const request: Extract<Event["event"], { kind: "question" }> = {
  kind: "question",
  request_id: "ask",
  tool_use_id: "tool",
  questions: [
    {
      id: "q0",
      text: "选择输出方式",
      header: "方式",
      multi_select: false,
      options: [
        { label: "简短", description: "摘要" },
        { label: "详细", description: "完整内容" },
      ],
    },
    {
      id: "q1",
      text: "选择章节",
      header: "章节",
      multi_select: true,
      options: [
        { label: "开头", description: "介绍" },
        { label: "结论", description: "总结" },
      ],
    },
  ],
};
it("requires explicit answers, supports single/multi selection and custom text without a permission gate", () => {
  const answer = vi.fn();
  render(<QuestionCard request={request} onAnswer={answer} />);
  expect(
    (screen.getByRole("button", { name: "提交回答" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  expect(screen.queryByText(/是否允许|允许一次|拒绝/)).toBeNull();
  fireEvent.click(screen.getByRole("radio", { name: /详细/ }));
  fireEvent.click(screen.getByRole("checkbox", { name: /开头/ }));
  fireEvent.click(screen.getByRole("checkbox", { name: /结论/ }));
  fireEvent.click(screen.getByRole("checkbox", { name: /其他/ }));
  expect(
    (screen.getByRole("button", { name: "提交回答" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  fireEvent.change(screen.getByRole("textbox", { name: "章节的自填回答" }), {
    target: { value: "  附录  " },
  });
  fireEvent.click(screen.getByRole("button", { name: "提交回答" }));
  expect(answer).toHaveBeenCalledWith([
    { question_id: "q0", option_indices: [1], text: null },
    { question_id: "q1", option_indices: [0, 1], text: "附录" },
  ]);
  fireEvent.click(screen.getByRole("radio", { name: /其他/ }));
  fireEvent.change(screen.getByRole("textbox", { name: "方式的自填回答" }), {
    target: { value: "先给目录" },
  });
  fireEvent.click(screen.getByRole("button", { name: "提交回答" }));
  expect(answer.mock.lastCall?.[0][0]).toEqual({
    question_id: "q0",
    option_indices: [],
    text: "先给目录",
  });
});
it("preserves choices while disabled, does not submit twice while pending, and skips without invented answers", () => {
  const answer = vi.fn();
  const view = render(<QuestionCard request={request} onAnswer={answer} />);
  fireEvent.click(screen.getByRole("radio", { name: /简短/ }));
  view.rerender(<QuestionCard request={request} onAnswer={answer} busy />);
  expect(
    (screen.getByRole("radio", { name: /简短/ }) as HTMLInputElement).checked,
  ).toBe(true);
  expect(
    (
      screen
        .getByRole("radio", { name: /简短/ })
        .closest("fieldset") as HTMLFieldSetElement
    ).disabled,
  ).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "跳过" }));
  expect(answer).not.toHaveBeenCalled();
  view.rerender(<QuestionCard request={request} onAnswer={answer} />);
  fireEvent.click(screen.getByRole("button", { name: "跳过" }));
  expect(answer).toHaveBeenCalledWith(null);
});
it("shows saved answers, skipped and expired states without live controls or HTML execution", () => {
  const view = render(
    <QuestionCard
      request={request}
      resolved
      answers={[
        { question_id: "q0", option_indices: [1], text: null },
        {
          question_id: "q1",
          option_indices: [],
          text: "<script>danger</script>",
        },
      ]}
      onAnswer={vi.fn()}
    />,
  );
  expect(screen.getByText("已回答")).toBeTruthy();
  expect(screen.getByText("<script>danger</script>")).toBeTruthy();
  expect(document.querySelector("script")).toBeNull();
  expect(screen.queryByRole("button", { name: "提交回答" })).toBeNull();
  view.rerender(
    <QuestionCard
      request={request}
      resolved
      answers={null}
      onAnswer={vi.fn()}
    />,
  );
  expect(screen.getByText("已跳过")).toBeTruthy();
  view.rerender(<QuestionCard request={request} resolved onAnswer={vi.fn()} />);
  expect(screen.getByText("问题已过期")).toBeTruthy();
});
