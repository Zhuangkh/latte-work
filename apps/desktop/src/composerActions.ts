import type { AgentSlashCommand } from "./protocol";

export { referencePrompt } from "./referenceMessage";

export interface ComposerTrigger {
  kind: "files" | "commands";
  start: number;
  end: number;
  query: string;
}
export function composerTrigger(
  text: string,
  cursor: number,
): ComposerTrigger | null {
  const prefix = text.slice(0, cursor);
  if (/^\/[^\s/]*$/.test(prefix))
    return { kind: "commands", start: 0, end: cursor, query: prefix.slice(1) };
  const match = /(?:^|\s)@([^\n@]*)$/.exec(prefix);
  if (!match) return null;
  return {
    kind: "files",
    start: cursor - match[1].length - 1,
    end: cursor,
    query: match[1],
  };
}
export const subagentPanelAliases = [
  "agents",
  "list-agents",
  "tasks",
  "subagents",
];
export const subagentPanelArgumentNotice =
  "已打开子智能体面板；此命令不接受参数，输入内容已保留。";

export function isSubagentPanelCommand(text: string) {
  const name = /^\/([^\s]+)(?:\s|$)/.exec(text.trim())?.[1];
  return !!name && subagentPanelAliases.includes(name);
}

export function hasCommandArguments(text: string) {
  return /^\/[^\s]+\s+\S/.test(text.trim());
}

export function isNativeSubagentCommand(command: AgentSlashCommand) {
  return (
    command.ui_action === "subagents" ||
    subagentPanelAliases.includes(command.name)
  );
}

export type AppCommandAction = "subagents" | "model" | "effort";
export type ComposerCommand = Omit<AgentSlashCommand, "ui_action"> & {
  ui_action?: AppCommandAction;
};

/** App-owned names take precedence even with an older or unavailable catalog. */
export function appCommandAction(
  text: string,
  commands: AgentSlashCommand[] = [],
): AppCommandAction | null {
  const name = /^\/([^\s]+)(?:\s|$)/.exec(text.trim())?.[1];
  if (!name) return null;
  if (name === "model" || name === "effort") return name;
  if (subagentPanelAliases.includes(name)) return "subagents";
  return commands.find((command) => command.name === name)?.ui_action ?? null;
}

export function unifiedComposerCommands(
  commands: AgentSlashCommand[],
): ComposerCommand[] {
  return [
    {
      name: "agents",
      display_name: "子智能体",
      description: "打开 Latte Work 子智能体面板",
      argument_hint: "",
      ui_action: "subagents",
    },
    {
      name: "model",
      display_name: "选择模型",
      description: "打开 Latte Work 模型菜单",
      argument_hint: "",
      ui_action: "model",
    },
    {
      name: "effort",
      display_name: "思考强度",
      description: "打开 Latte Work 思考强度菜单",
      argument_hint: "",
      ui_action: "effort",
    },
    ...commands.filter(
      (command) =>
        !isNativeSubagentCommand(command) &&
        command.name !== "model" &&
        command.name !== "effort",
    ),
  ];
}

const claudeCommandTitles: Record<string, string> = {
  compact: "压缩上下文",
  context: "上下文用量",
  usage: "使用情况",
  cost: "使用费用",
  clear: "清空上下文",
  init: "初始化项目",
  model: "选择模型",
  effort: "思考强度",
  mcp: "MCP 服务",
  config: "配置",
  fast: "快速模式",
  rename: "重命名会话",
  "output-style": "输出风格",
  "security-review": "安全审查",
  "code-review": "代码审查",
  "reload-plugins": "重新加载插件",
  "reload-skills": "重新加载技能",
  insights: "使用洞察",
  recap: "会话回顾",
  goal: "设置目标",
};
export function commandTitle(
  command: Omit<AgentSlashCommand, "ui_action">,
  agent: string,
): string {
  const label = command.display_name?.trim();
  if (label && label !== command.name) return label;
  if (agent === "claude" && claudeCommandTitles[command.name])
    return claudeCommandTitles[command.name];
  // Some skills expose their human title at the beginning of the description.
  // Use only a short explicit title; do not turn arbitrary prose into a label.
  const title = /^([^：:\n]{2,28})[：:]/u
    .exec(command.description)?.[1]
    ?.trim();
  if (title) return title;
  return command.name
    .split(/[-_:]+/)
    .filter(Boolean)
    .map((word) => word[0].toUpperCase() + word.slice(1))
    .join(" ");
}
