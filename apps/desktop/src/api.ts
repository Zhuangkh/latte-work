import {
  HOST_DISCONNECTED_EVENT,
  HOST_CONNECTION_PREFIX,
  HostConnectionError,
} from "./connectionErrors";
import { invoke } from "@tauri-apps/api/core";
import type { Request, Response } from "./protocol";
export interface Host {
  id: string;
  name: string;
  ssh: string | null;
  server_path: string | null;
  platform?: "unix" | "windows";
  auth?: "none" | "identity_file" | "password";
  identity_file?: string | null;
  port?: number | null;
}
export const localHost: Host = {
  id: "local",
  name: "本机",
  ssh: null,
  server_path: null,
};
export const native = "__TAURI_INTERNALS__" in window;
const connectionGenerations = new Map<string, number>();
const connections = new Map<
  string,
  { signature: string; promise: Promise<Response> }
>();
export function connect(host: Host, password?: string): Promise<Response> {
  const signature = JSON.stringify(host);
  const pending = connections.get(host.id);
  if (pending?.signature === signature && password === undefined)
    return pending.promise;
  const invokeConnection = () => {
    connectionGenerations.set(
      host.id,
      (connectionGenerations.get(host.id) ?? 0) + 1,
    );
    return invoke<Response>("connect_host", { host, password });
  };
  const promise = (
    pending
      ? pending.promise.catch(() => undefined).then(invokeConnection)
      : invokeConnection()
  ).finally(() => {
    if (connections.get(host.id)?.promise === promise)
      connections.delete(host.id);
  });
  connections.set(host.id, { signature, promise });
  return promise;
}
export function disconnect(hostId: string): Promise<void> {
  connectionGenerations.set(
    hostId,
    (connectionGenerations.get(hostId) ?? 0) + 1,
  );
  return invoke("disconnect_host", { hostId });
}
export interface ProjectFolder {
  path: string;
  name: string;
}
export function chooseProjectFolder(): Promise<ProjectFolder | null> {
  return invoke("choose_project_folder");
}
export function chooseReferencePath(
  directory: boolean,
): Promise<string | null> {
  return invoke("choose_reference_path", { directory });
}
export function chooseIdentityFile(): Promise<string | null> {
  return invoke("choose_identity_file");
}
export async function request(
  hostId: string,
  request: Request,
): Promise<Response> {
  const generation = connectionGenerations.get(hostId);
  let response: Response;
  try {
    response = await invoke<Response>("host_request", { hostId, request });
  } catch (cause) {
    const detail = message(cause);
    if (!detail.startsWith(HOST_CONNECTION_PREFIX)) throw cause;
    const error = new HostConnectionError(
      hostId,
      detail.slice(HOST_CONNECTION_PREFIX.length),
    );
    if (generation === connectionGenerations.get(hostId))
      window.dispatchEvent(
        new CustomEvent(HOST_DISCONNECTED_EVENT, { detail: error }),
      );
    throw error;
  }
  if (response.kind === "error") throw new Error(response.message);
  return response;
}
export async function loadHosts(): Promise<Host[]> {
  return [localHost, ...(await invoke<Host[]>("load_hosts"))];
}
export async function saveHosts(hosts: Host[]): Promise<void> {
  return invoke("save_hosts", { hosts: hosts.filter((h) => h.id !== "local") });
}
export function message(error: unknown): string {
  const value = error instanceof Error ? error.message : String(error);
  return value === "SSH_PASSWORD_REQUIRED" ? "请输入 SSH 密码后连接" : value;
}

export function revealProject(
  hostId: string,
  projectId: string,
): Promise<void> {
  return invoke("reveal_project", { hostId, projectId });
}

export async function bindAgentProvider(
  hostId: string,
  agent: string,
  providerId: string | null,
): Promise<Response> {
  const response = await invoke<Response>("bind_agent_provider", {
    hostId,
    agent,
    providerId,
  });
  if (response.kind === "error") throw new Error(response.message);
  return response;
}

export async function agentProviders(hostId: string): Promise<Response> {
  const response = await invoke<Response>("agent_providers", { hostId });
  if (response.kind === "error") throw new Error(response.message);
  return response;
}

export type ProviderRequest = Extract<
  Request,
  { method: "providers" | "save_provider" | "delete_provider" }
>;
export async function providerRequest(
  request: ProviderRequest,
): Promise<Response> {
  const response = await invoke<Response>("provider_request", { request });
  if (response.kind === "error") throw new Error(response.message);
  return response;
}

export async function fetchProviderModels(
  provider: import("./protocol").ProviderDraft,
): Promise<{ id: string; name: string }[]> {
  return invoke("fetch_provider_models", { provider });
}
