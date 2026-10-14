import { useEffect, useRef, useState } from "react";
import {
  Archive,
  ArrowLeft,
  Globe2,
  MessageSquare,
  Settings2,
  Palette,
} from "lucide-react";
import { ArchivedChats } from "./ArchivedChats";
import { AgentSettings } from "./AgentSettings";
import { ProviderSettings } from "./ProviderSettings";
import { WindowControls } from "./WindowControls";
import { SSHSettings } from "./SSHSettings";
import type { Workbench } from "./useWorkbench";

import { BrowserSettings } from "./BrowserSettings";
import { AppearanceSettings } from "./AppearanceSettings";

const tabs = [
  { id: "browser", label: "浏览器", icon: Globe2 },
  { id: "appearance", label: "外观", icon: Palette },
  { id: "agents", label: "连接与 Agent", icon: MessageSquare },
  { id: "providers", label: "模型", icon: Settings2 },
  { id: "ssh", label: "SSH 连接", icon: Globe2 },
  { id: "archived", label: "已归档的聊天", icon: Archive },
] as const;

export function SettingsPage({
  state,
  active,
}: {
  state: Workbench;
  active: boolean;
}) {
  const tab = state.settingsTab;
  const setTab = state.setSettingsTab;
  const [agentBusy, setAgentBusy] = useState(false);
  const [providerBusy, setProviderBusy] = useState(false);
  const [sshBusy, setSshBusy] = useState(false);
  const navigation = useRef<HTMLDivElement>(null);
  const [archiveBusy, setArchiveBusy] = useState(false);
  const busy = agentBusy || providerBusy || sshBusy || archiveBusy;
  useEffect(() => {
    if (!active) return;
    navigation.current
      ?.querySelector<HTMLButtonElement>(`#settings-tab-${tab}`)
      ?.focus();
  }, [active]);
  useEffect(() => {
    if (!active) return;
    const key = (event: KeyboardEvent) => {
      if (
        !event.defaultPrevented &&
        event.key === "Escape" &&
        !busy &&
        !document.querySelector(".ssh-password-modal") &&
        !document.querySelector(".ssh-modal")
      )
        close();
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [active, busy, state.setModal, state.settingsReturnToProject]);
  function close() {
    if (!busy) {
      state.setModal(state.settingsReturnToProject ? "project" : null);
      state.setSettingsReturnToProject(false);
    }
  }
  return (
    <div className="settings-page">
      <div className="settings-window-drag" data-tauri-drag-region>
        <WindowControls />
      </div>
      <div className="settings-layout">
        <aside className="settings-navigation">
          <h1>设置</h1>
          <button className="settings-back" onClick={close} disabled={busy}>
            <ArrowLeft size={16} />
            返回工作台
          </button>
          <div
            ref={navigation}
            role="tablist"
            aria-label="设置分类"
            aria-orientation="vertical"
            onKeyDown={(event) => {
              if (
                busy ||
                !["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)
              )
                return;
              event.preventDefault();
              const current = tabs.findIndex((entry) => entry.id === tab);
              const next =
                event.key === "Home"
                  ? 0
                  : event.key === "End"
                    ? tabs.length - 1
                    : (current +
                        (event.key === "ArrowDown" ? 1 : -1) +
                        tabs.length) %
                      tabs.length;
              setTab(tabs[next].id);
              if (!active) return;
              navigation.current
                ?.querySelectorAll<HTMLButtonElement>("button")
                [next]?.focus();
            }}
          >
            {tabs.map(({ id, label, icon: Icon }) => (
              <button
                key={id}
                id={`settings-tab-${id}`}
                role="tab"
                aria-selected={tab === id}
                aria-controls={`settings-panel-${id}`}
                tabIndex={tab === id ? 0 : -1}
                disabled={busy}
                onClick={() => setTab(id)}
              >
                <Icon size={17} />
                {label}
              </button>
            ))}
          </div>
        </aside>
        <main className="settings-body" aria-label="设置">
          <section
            id="settings-panel-browser"
            role="tabpanel"
            aria-labelledby="settings-tab-browser"
            hidden={tab !== "browser"}
          >
            <BrowserSettings />
          </section>
          <section
            id="settings-panel-archived"
            role="tabpanel"
            aria-labelledby="settings-tab-archived"
            hidden={tab !== "archived"}
          >
            <ArchivedChats
              state={state}
              active={active && tab === "archived"}
              onBusy={setArchiveBusy}
            />
          </section>
          <section
            id="settings-panel-appearance"
            role="tabpanel"
            aria-labelledby="settings-tab-appearance"
            hidden={tab !== "appearance"}
          >
            {tab === "appearance" && <AppearanceSettings />}
          </section>
          <section
            id="settings-panel-agents"
            role="tabpanel"
            aria-labelledby="settings-tab-agents"
            hidden={tab !== "agents"}
          >
            <AgentSettings
              state={state}
              active={active && tab === "agents"}
              onBusy={setAgentBusy}
            />
          </section>
          <section
            id="settings-panel-providers"
            role="tabpanel"
            aria-labelledby="settings-tab-providers"
            hidden={tab !== "providers"}
          >
            <ProviderSettings
              active={active && tab === "providers"}
              onBusy={setProviderBusy}
            />
          </section>
          <section
            id="settings-panel-ssh"
            role="tabpanel"
            aria-labelledby="settings-tab-ssh"
            hidden={tab !== "ssh"}
          >
            <SSHSettings state={state} onBusy={setSshBusy} />
          </section>
        </main>
      </div>
    </div>
  );
}
