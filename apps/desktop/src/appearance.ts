export type ThemeMode = "system" | "light" | "dark";
export type ThemeKind = "light" | "dark";
export type FontChoice = "system" | "sans" | "serif" | "mono";
export type FontWeight = "400" | "500" | "600";
export interface ThemeConfig {
  accent: string;
  background: string;
  foreground: string;
  contrast: number;
  translucentSidebar: boolean;
  uiFont: FontChoice;
  contentFont: FontChoice | "inherit";
  codeFont: "system" | "menlo" | "consolas";
  uiWeight: FontWeight;
  contentWeight: FontWeight;
  codeWeight: FontWeight;
}
export interface Appearance {
  version: 1;
  mode: ThemeMode;
  light: ThemeConfig;
  dark: ThemeConfig;
}
export const APPEARANCE_KEY = "latte-work.appearance.v1";
export const APPEARANCE_EVENT = "latte-work:appearance";
export const MAX_THEME_BYTES = 32 * 1024;
export const fonts = {
  system:
    '-apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", sans-serif',
  sans: 'Arial, "PingFang SC", sans-serif',
  serif: 'Georgia, "Songti SC", serif',
  mono: 'ui-monospace, "SFMono-Regular", Menlo, monospace',
};
export const codeFonts = {
  system: fonts.mono,
  menlo: 'Menlo, Monaco, "Courier New", monospace',
  consolas: 'Consolas, "Courier New", monospace',
};
export const presets: Record<
  ThemeKind,
  Record<string, Pick<ThemeConfig, "accent" | "background" | "foreground">>
> = {
  dark: {
    Latte: { accent: "#F1F1F1", background: "#111111", foreground: "#EEEEEE" },
    Material: {
      accent: "#82AAFF",
      background: "#212D33",
      foreground: "#FFFFFF",
    },
    Graphite: {
      accent: "#B9A3FF",
      background: "#222225",
      foreground: "#ECECF0",
    },
  },
  light: {
    Latte: { accent: "#2563EB", background: "#FFFFFF", foreground: "#222222" },
    Paper: { accent: "#806044", background: "#FAF7F2", foreground: "#302C28" },
    Mint: { accent: "#247564", background: "#F1F7F5", foreground: "#20352F" },
  },
};
export function defaultTheme(kind: ThemeKind): ThemeConfig {
  return {
    ...presets[kind].Latte,
    contrast: 50,
    translucentSidebar: false,
    uiFont: "system",
    contentFont: "inherit",
    codeFont: "system",
    uiWeight: "400",
    contentWeight: "400",
    codeWeight: "400",
  };
}
export function defaultAppearance(): Appearance {
  return {
    version: 1,
    mode: "system",
    light: defaultTheme("light"),
    dark: defaultTheme("dark"),
  };
}
export function isColor(value: unknown): value is string {
  return typeof value === "string" && /^#[\da-f]{6}$/i.test(value);
}
function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function choice(value: unknown, choices: string[]): boolean {
  return typeof value === "string" && choices.includes(value);
}
function parseTheme(value: unknown): ThemeConfig {
  if (
    !record(value) ||
    ![value.accent, value.background, value.foreground].every(isColor) ||
    typeof value.contrast !== "number" ||
    !Number.isFinite(value.contrast) ||
    value.contrast < 0 ||
    value.contrast > 100 ||
    typeof value.translucentSidebar !== "boolean" ||
    !choice(value.uiFont, ["system", "sans", "serif", "mono"]) ||
    !choice(value.contentFont, [
      "inherit",
      "system",
      "sans",
      "serif",
      "mono",
    ]) ||
    !choice(value.codeFont, ["system", "menlo", "consolas"]) ||
    ![value.uiWeight, value.contentWeight, value.codeWeight].every(
      (v) => typeof v === "string" && ["400", "500", "600"].includes(v),
    )
  ) {
    throw new Error("主题字段无效，请使用从 Latte Work 复制的主题 JSON。");
  }
  // Copy only known fields; imported data is never interpreted as CSS or HTML.
  return {
    accent: String(value.accent).toUpperCase(),
    background: String(value.background).toUpperCase(),
    foreground: String(value.foreground).toUpperCase(),
    contrast: value.contrast,
    translucentSidebar: value.translucentSidebar,
    uiFont: value.uiFont as FontChoice,
    contentFont: value.contentFont as ThemeConfig["contentFont"],
    codeFont: value.codeFont as ThemeConfig["codeFont"],
    uiWeight: value.uiWeight as FontWeight,
    contentWeight: value.contentWeight as FontWeight,
    codeWeight: value.codeWeight as FontWeight,
  };
}
export function parseAppearance(text: string): Appearance {
  if (new TextEncoder().encode(text).length > MAX_THEME_BYTES)
    throw new Error("主题文件不能超过 32 KB。");
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    throw new Error("JSON 格式不正确。");
  }
  if (
    !record(value) ||
    value.version !== 1 ||
    !choice(value.mode, ["system", "light", "dark"])
  ) {
    throw new Error("不支持的主题格式或版本。");
  }
  return {
    version: 1,
    mode: value.mode as ThemeMode,
    light: parseTheme(value.light),
    dark: parseTheme(value.dark),
  };
}
export function readAppearance(): { settings: Appearance; error: string } {
  try {
    const raw = localStorage.getItem(APPEARANCE_KEY);
    return {
      settings: raw ? parseAppearance(raw) : defaultAppearance(),
      error: "",
    };
  } catch {
    return {
      settings: defaultAppearance(),
      error: "无法读取已保存的外观，当前使用默认主题。重新调整后可保存。",
    };
  }
}
export function saveAppearance(settings: Appearance): string {
  try {
    localStorage.setItem(APPEARANCE_KEY, JSON.stringify(settings));
    return "";
  } catch {
    return "外观已应用，但无法保存到本机。重启后可能丢失。";
  }
}
export function resolveTheme(mode: ThemeMode, systemDark: boolean): ThemeKind {
  return mode === "system" ? (systemDark ? "dark" : "light") : mode;
}
function rgb(color: string): number[] {
  return [1, 3, 5].map((i) => parseInt(color.slice(i, i + 2), 16));
}
export function mix(a: string, b: string, ratio: number): string {
  const x = rgb(a),
    y = rgb(b);
  return (
    "#" +
    x
      .map((v, i) =>
        Math.round(v + (y[i] - v) * ratio)
          .toString(16)
          .padStart(2, "0"),
      )
      .join("")
  );
}
function luminance(color: string): number {
  const values = rgb(color).map((v) => {
    const n = v / 255;
    return n <= 0.04045 ? n / 12.92 : ((n + 0.055) / 1.055) ** 2.4;
  });
  return values[0] * 0.2126 + values[1] * 0.7152 + values[2] * 0.0722;
}
export function contrastRatio(a: string, b: string): number {
  const l = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (l[0] + 0.05) / (l[1] + 0.05);
}
export function themeTokens(theme: ThemeConfig): Record<string, string> {
  const bg = theme.background,
    fg = theme.foreground,
    accent = theme.accent;
  const dark = luminance(bg) < 0.4;
  const depth = 0.025 + (theme.contrast / 100) * 0.11;
  const surface = (n: number) => mix(bg, fg, n);
  const sidebar = surface(depth * (dark ? 0.5 : 0.55));
  return {
    "--color-canvas": bg,
    "--color-text": fg,
    "--color-sidebar": theme.translucentSidebar ? sidebar + "c7" : sidebar,
    "--color-surface": surface(dark ? depth : depth * 0.65),
    "--color-elevated": dark ? surface(depth) : bg,
    "--color-input": surface(depth * 0.25),
    "--color-composer": dark ? surface(depth + 0.025) : bg,
    "--color-text-secondary": surface(0.78),
    "--color-text-muted": surface(dark ? 0.62 : 0.65),
    "--color-placeholder": surface(dark ? 0.62 : 0.65),
    "--color-border-subtle": surface(dark ? depth + 0.02 : depth * 0.6),
    "--color-border": surface(dark ? depth + 0.06 : depth + 0.02),
    "--color-border-strong": surface(dark ? depth + 0.06 : depth + 0.02),
    "--color-hover": fg + (dark ? "12" : "09"),
    "--color-selected": fg + (dark ? "20" : "16"),
    "--color-focus-background": fg + "29",
    "--color-primary": accent,
    "--color-primary-hover": mix(accent, dark ? "#FFFFFF" : "#000000", 0.1),
    "--color-on-primary":
      contrastRatio(accent, "#FFFFFF") > contrastRatio(accent, "#111111")
        ? "#FFFFFF"
        : "#111111",
    "--color-action": dark ? "#FFFFFF" : "#222222",
    "--color-action-hover": dark ? "#EAEAEA" : "#383838",
    "--color-on-action": dark ? "#222222" : "#FFFFFF",
    "--color-disabled-bg": surface(0.055),
    "--color-disabled-text": surface(dark ? 0.62 : 0.65),
    "--color-stop": surface(dark ? 0.22 : 0.12),
    "--color-stop-hover": surface(dark ? 0.3 : 0.2),
    "--color-on-stop": fg,
    "--color-brand": accent,
    "--color-code-keyword": dark ? "#CDA4D7" : "#6B3D83",
    "--color-code-string": dark ? "#C4C998" : "#526B20",
    "--color-code-number": dark ? "#A7C6BE" : "#256557",
    "--color-code-type": dark ? "#DFBC85" : "#805616",
    "--color-success": dark ? "#75C99A" : "#237A48",
    "--color-warning": dark ? "#E4B66A" : "#946000",
    "--color-danger": dark ? "#EF9090" : "#C03939",
    "--color-unread": dark ? "#85AAFF" : "#2563EB",
    "--color-backdrop": dark ? "#000000a6" : "#00000040",
    "--color-selection": accent + "44",
    "--shadow-dialog": dark ? "0 24px 72px #0005" : "0 24px 72px #00000014",
    "--shadow-popover": dark ? "0 12px 36px #0006" : "0 8px 28px #0000000a",
    "--font-ui": fonts[theme.uiFont],
    "--font-content":
      theme.contentFont === "inherit"
        ? fonts[theme.uiFont]
        : fonts[theme.contentFont],
    "--font-code": codeFonts[theme.codeFont],
    "--weight-ui": theme.uiWeight,
    "--weight-content": theme.contentWeight,
    "--weight-content-emphasis": theme.contentWeight === "400" ? "500" : "600",
    "--weight-code": theme.codeWeight,
  };
}
export function applyAppearance(
  settings: Appearance,
  systemDark: boolean,
): void {
  const kind = resolveTheme(settings.mode, systemDark);
  const root = document.documentElement;
  Object.entries(themeTokens(settings[kind])).forEach(([key, value]) =>
    root.style.setProperty(key, value),
  );
  root.style.colorScheme = kind;
  root.dataset.theme = kind;
  root.dataset.translucentSidebar = String(settings[kind].translucentSidebar);
  window.dispatchEvent(new Event(APPEARANCE_EVENT));
}
