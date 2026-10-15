import { useState } from "react";
import { ChevronRight, MessageCircleQuestion } from "lucide-react";
import type { Event, QuestionAnswer } from "./protocol";

type QuestionRequest = Extract<Event["event"], { kind: "question" }>;
type Draft = { selected: number[]; custom: boolean; text: string };
export function QuestionCard({
  request,
  resolved,
  answers,
  disabled,
  busy,
  record = false,
  onAnswer,
}: {
  request: QuestionRequest;
  resolved?: boolean;
  answers?: QuestionAnswer[] | null;
  disabled?: boolean;
  busy?: boolean;
  record?: boolean;
  onAnswer: (answers: QuestionAnswer[] | null) => void;
}) {
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  function draft(id: string): Draft {
    return drafts[id] ?? { selected: [], custom: false, text: "" };
  }
  function update(id: string, change: (value: Draft) => Draft) {
    setDrafts((old) => ({
      ...old,
      [id]: change(old[id] ?? { selected: [], custom: false, text: "" }),
    }));
  }
  const result: QuestionAnswer[] = request.questions.map((question) => ({
    question_id: question.id,
    option_indices: draft(question.id).selected,
    text: draft(question.id).custom
      ? draft(question.id).text.trim() || null
      : null,
  }));
  const valid = result.every((answer, index) => {
    const count = answer.option_indices.length + Number(!!answer.text);
    return (
      count > 0 &&
      (request.questions[index].multi_select || count === 1) &&
      (!draft(answer.question_id).custom || !!answer.text)
    );
  });
  if (record || resolved)
    return (
      <details className="question-result activity-row">
        <summary>
          <MessageCircleQuestion size={16} />
          <span className="activity-label">回答问题</span>
          <span className="activity-status">
            {!resolved
              ? "等待回答"
              : answers === undefined
                ? "问题已过期"
                : answers === null
                  ? "已跳过"
                  : "已回答"}
          </span>
          <ChevronRight size={14} className="activity-chevron" />
        </summary>
        <div className="question-record">
          {request.questions.map((question) => {
            const answer = answers?.find(
              (answer) => answer.question_id === question.id,
            );
            return (
              <div key={question.id}>
                <p>{question.text}</p>
                {answer && (
                  <p className="question-record-answer">
                    {[
                      ...answer.option_indices.map(
                        (index) => question.options[index]?.label ?? "",
                      ),
                      answer.text,
                    ]
                      .filter(Boolean)
                      .join("、")}
                  </p>
                )}
              </div>
            );
          })}
        </div>
      </details>
    );
  return (
    <form
      className="approval-card question-card"
      aria-label="待回答问题"
      onSubmit={(event) => {
        event.preventDefault();
        if (valid && !disabled && !busy) onAnswer(result);
      }}
    >
      <div className="approval-category">
        <MessageCircleQuestion size={16} />
        <span>需要你的回答</span>
      </div>
      {request.questions.map((question) => (
        <fieldset key={question.id} disabled={disabled || busy}>
          <legend>{question.text}</legend>
          {question.multi_select && (
            <p className="question-selection-hint">可选择多项</p>
          )}
          <div className="question-choices">
            {question.options.map((option, index) => (
              <label key={index} className="question-choice">
                <input
                  type={question.multi_select ? "checkbox" : "radio"}
                  name={`${request.request_id}:${question.id}`}
                  checked={draft(question.id).selected.includes(index)}
                  onChange={() =>
                    update(question.id, (old) =>
                      question.multi_select
                        ? {
                            ...old,
                            selected: old.selected.includes(index)
                              ? old.selected.filter((i) => i !== index)
                              : [...old.selected, index],
                          }
                        : { ...old, selected: [index], custom: false },
                    )
                  }
                />
                <span>
                  <strong>{option.label}</strong>
                  {option.description && <small>{option.description}</small>}
                </span>
              </label>
            ))}
            <label className="question-choice">
              <input
                type={question.multi_select ? "checkbox" : "radio"}
                name={`${request.request_id}:${question.id}`}
                checked={draft(question.id).custom}
                onChange={() =>
                  update(question.id, (old) => ({
                    ...old,
                    custom: question.multi_select ? !old.custom : true,
                    selected: question.multi_select ? old.selected : [],
                  }))
                }
              />
              <span>
                <strong>其他</strong>
                <small>填写自己的回答</small>
              </span>
            </label>
          </div>
          {draft(question.id).custom && (
            <textarea
              aria-label={`${question.header || question.text}的自填回答`}
              placeholder="填写回答…"
              maxLength={8000}
              value={draft(question.id).text}
              onChange={(event) =>
                update(question.id, (old) => ({
                  ...old,
                  text: event.target.value,
                }))
              }
            />
          )}
        </fieldset>
      ))}
      <div className="approval-actions">
        <button
          type="button"
          className="secondary"
          disabled={disabled || busy}
          onClick={() => onAnswer(null)}
        >
          跳过
        </button>
        <button
          type="submit"
          className="primary"
          disabled={disabled || busy || !valid}
        >
          {busy ? "正在提交…" : "提交回答"}
        </button>
      </div>
    </form>
  );
}
