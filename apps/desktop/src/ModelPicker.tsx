import {
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
  type Ref,
} from "react";
import { request, message } from "./api";
import { modelOptions } from "./modelOptions";
import { modelSelectionScope } from "./modelSelectionScope";
import { ModelSelector, type ModelSelectorHandle } from "./ModelSelector";
import type { Effort, Response } from "./protocol";
import { useStatusIssues } from "./statusNotices";

export interface ModelPickerHandle {
  /** Null means opened; an error preserves the command draft. */
  openMenu: (target: "model" | "effort") => string | null;
}

type Catalog = Extract<Response, { kind: "models" }>;
export function ModelPicker({
  hostId,
  projectId,
  agent,
  connected,
  settingsOpen,
  value,
  onChange,
  effort,
  onEffortChange,
  onEffortInvalid,
  disabled,
  onReadyChange,
  ref,
}: {
  hostId: string;
  projectId?: string;
  agent: string;
  connected: boolean;
  settingsOpen: boolean;
  value: string | null;
  onChange: (model: string | null) => void;
  effort: Effort | null;
  onEffortChange: (effort: Effort | null) => void;
  onEffortInvalid?: () => void;
  disabled: boolean;
  onReadyChange?: (state: { scope: string; ready: boolean }) => void;
  ref?: Ref<ModelPickerHandle>;
}) {
  const selector = useRef<ModelSelectorHandle>(null);
  const scope = JSON.stringify([hostId, projectId, agent, value]);
  const [loaded, setCatalog] = useState<{ scope: string; catalog: Catalog }>();
  const catalog = loaded?.scope === scope ? loaded.catalog : undefined;
  const [error, setError] = useState("");
  const [retry, setRetry] = useState(0);
  const [loading, setLoading] = useState(false);
  useStatusIssues([
    {
      id: `models:${hostId}:${projectId}:${agent}`,
      title: "模型列表暂不可用",
      error: connected && !settingsOpen ? error : "",
      pending: connected && !settingsOpen && loading,
      action: { label: "重新加载模型", run: () => setRetry((v) => v + 1) },
    },
  ]);
  useEffect(() => {
    if (!connected || settingsOpen) return;
    let disposed = false;
    setCatalog(undefined);
    setError("");
    setLoading(true);
    void request(hostId, {
      method: "models",
      agent,
      model: value,
      project_id: projectId ?? null,
    })
      .then((result) => {
        if (result.kind !== "models") throw new Error("无法加载模型列表");
        if (!disposed) setCatalog({ scope, catalog: result });
      })
      .catch((e) => {
        if (!disposed) setError(message(e));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [hostId, projectId, agent, connected, settingsOpen, retry, value]);
  useEffect(() => {
    if (catalog && effort && !catalog.effort_levels.includes(effort))
      onEffortInvalid?.();
  }, [catalog, effort, onEffortInvalid]);
  const providerFallback =
    catalog?.provider != null && (!value || !catalog.models.includes(value))
      ? catalog.models.includes(catalog.default_model ?? "")
        ? catalog.default_model
        : (catalog.models[0] ?? null)
      : undefined;
  useEffect(() => {
    if (providerFallback !== undefined && providerFallback !== value)
      onChange(providerFallback);
  }, [providerFallback, value, onChange]);
  const readyScope = modelSelectionScope(
    hostId,
    projectId,
    agent,
    value,
    effort,
  );
  const ready =
    !!catalog &&
    providerFallback === undefined &&
    (!value || catalog.models.includes(value)) &&
    (!effort || catalog.effort_levels.includes(effort)) &&
    connected &&
    !settingsOpen;
  useEffect(() => {
    onReadyChange?.({ scope: readyScope, ready });
  }, [readyScope, ready, onReadyChange]);
  useImperativeHandle(
    ref,
    () => ({
      openMenu(target) {
        if (disabled) return "当前无法调整模型或思考强度，请在任务结束后选择。";
        if (!connected) return "主机未连接，请连接后选择模型或思考强度。";
        if (settingsOpen || !catalog) return "模型设置尚未就绪，请稍后重试。";
        if (target === "effort" && !catalog?.effort_levels.length)
          return "当前 Agent 或模型不支持调整思考强度。";
        return selector.current?.openMenu(target) ? null : "无法打开模型设置。";
      },
    }),
    [disabled, connected, settingsOpen, catalog],
  );
  const options = modelOptions(
    catalog?.models ?? [],
    catalog?.model_labels,
  ).filter((option) => catalog?.provider == null || option.value !== "");
  if (
    value &&
    catalog &&
    catalog.provider == null &&
    !catalog.models.includes(value)
  )
    options.push({
      value,
      label: `${value} (unavailable)`,
    });
  return (
    <div className="model-picker">
      <ModelSelector
        ref={selector}
        value={providerFallback ?? value ?? ""}
        options={options}
        onChange={(id) => onChange(id || null)}
        effort={effort}
        levels={catalog?.effort_levels ?? []}
        onEffortChange={onEffortChange}
        disabled={disabled || settingsOpen || !connected || !catalog}
      />
    </div>
  );
}
