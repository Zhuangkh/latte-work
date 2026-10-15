import type { PathSyntax } from "./protocol";

/** Display-only basename for manual input, using the execution host's syntax.
 * Native picker/browser names take precedence. This never validates or rewrites
 * the path submitted to the host; unknown syntax must not guess separators.
 */
export function projectFolderName(
  path: string,
  syntax: PathSyntax | undefined,
): string | undefined {
  if (!path) return undefined;
  if (syntax === "posix") {
    if (!path.startsWith("/")) return undefined;
    const parts = path.split("/").filter((part) => part && part !== ".");
    const name = parts.at(-1);
    return name === ".." ? undefined : (name ?? "/");
  }
  if (syntax === "windows") {
    // A display copy only. Preserve the original path for submission and tooltip.
    let native = path.replaceAll("\\", "/");
    if (native.startsWith("//?/UNC/")) native = `//${native.slice(8)}`;
    else if (native.startsWith("//?/")) native = native.slice(4);
    if (!/^[a-z]:\//i.test(native) && !/^\/\/[^/]+\/[^/]+/.test(native))
      return undefined;
    const parts = native.split("/").filter((part) => part && part !== ".");
    const name = parts.at(-1);
    if (name === "..") return undefined;
    return parts.length === 1 ? `${name}\\` : name;
  }
  return undefined;
}
