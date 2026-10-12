import { useState } from "react";
import { Select } from "./Select";
import { saveLinkTarget, useLinkTarget, type LinkTarget } from "./browserLinks";
import { message } from "./api";
export function BrowserSettings() {
  const target = useLinkTarget();
  const [error, setError] = useState("");
  return (
    <div className="browser-settings settings-panel-content">
      <h2>浏览器</h2>
      <label>链接默认打开方式</label>
      <Select
        label="链接默认打开方式"
        value={target}
        options={[
          {
            value: "internal",
            label: "内置浏览器",
            description: "在右侧工作区打开网页标签",
          },
          {
            value: "system",
            label: "系统默认浏览器",
            description: "使用系统设置的浏览器",
          },
        ]}
        onChange={(value) => {
          try {
            saveLinkTarget(value as LinkTarget);
            setError("");
          } catch (e) {
            setError(`设置保存失败：${message(e)}`);
          }
        }}
      />
      <p className="muted">右键点击链接，可单独选择打开方式或复制链接。</p>
      {error && <p role="alert">{error}</p>}
    </div>
  );
}
