// The typed view of a running PushOS.
//
// These mirror the control protocol exactly. Nothing here invents a concept the
// runtime does not have: the vocabulary, the gestures and the actions all come
// from PushOS, so Studio cannot offer something that will not work.

import { invoke } from "@tauri-apps/api/core";

import { usePreview } from "./bridge";

export type ParamValue = string | number | boolean | ParamValue[];

export interface BindingAddress {
  control: string;
  gesture: string;
  page?: string;
  workspace?: string;
}

export interface BindingSpec {
  control: string;
  gesture: string;
  page?: string;
  workspace?: string;
  action: string;
  target?: string;
  params?: Record<string, ParamValue>;
  label?: string;
  priority?: number;
}

/**
 * What the runtime is driving.
 *
 * Three states, not a boolean. A stand-in surface is neither attached nor
 * unattached, and calling it either would be untrue.
 */
export type SurfaceReport = "push2" | "simulated" | "absent";

export interface StatusReport {
  protocol: number;
  version: string;
  surface: SurfaceReport;
  page: string | null;
  workspace: string | null;
  config_root: string;
  binding_count: number;
}

export interface GridPosition {
  row: number;
  column: number;
}

export interface ControlInfo {
  id: string;
  kind: "pad" | "button" | "encoder" | "touch_strip";
  illuminated: boolean;
  grid?: GridPosition;
}

export interface PageInfo {
  id: string;
  name: string;
  description?: string;
}

export interface ProviderInfo {
  name: string;
  verbs: string[];
  requires: string[];
  permitted: boolean;
}

export interface Vocabulary {
  controls: ControlInfo[];
  gestures: string[];
  pages: PageInfo[];
  providers: ProviderInfo[];
}

/** Whether a session is an agent or a terminal. */
export type SessionKind = "agent" | "terminal";

/**
 * One session running now, described so a control can be bound to it.
 *
 * Both target forms arrive because they are not interchangeable. `target`
 * names this exact session and stops meaning anything when it ends;
 * `standing_target` names the role or the terminal name, and goes on meaning
 * the right thing tomorrow.
 */
export interface SessionInfo {
  id: string;
  kind: SessionKind;
  name: string;
  status: string;
  live: boolean;
  selected: boolean;
  target: string;
  standing_target?: string;
  detail?: string;
  workspace?: string;
}

/**
 * One project.
 *
 * A workspace is what a pad means when it says "Sydclaw": where work happens,
 * what opens it, and what the surface shows on arrival.
 */
export interface WorkspaceInfo {
  id: string;
  name: string;
  root: string;
  description?: string;
  home_page?: string;
  current: boolean;
  isolate_agents: boolean;
  apps?: string[];
  root_exists: boolean;
}

/** One page, as an editor writes it. */
export interface PageSpec {
  id: string;
  name: string;
  description?: string;
}

export interface EditReport {
  file: string;
  replaced: boolean;
  binding_count: number;
  page_count: number;
  /** How many bindings the edit took with it, when it took any. */
  bindings_removed?: number;
}

export interface TestReport {
  action: string;
  status: string;
  message?: string;
}

/** What a command reports when PushOS cannot be reached or refuses. */
export interface StudioError {
  message: string;
  problems: string[];
  not_running: boolean;
}

/** Whether a thrown value is a failure the backend produced. */
export function isStudioError(value: unknown): value is StudioError {
  return (
    typeof value === "object" &&
    value !== null &&
    "message" in value &&
    "not_running" in value
  );
}

/** Turns anything thrown into something worth showing a person. */
export function describeError(error: unknown): StudioError {
  if (isStudioError(error)) return error;
  return {
    message: error instanceof Error ? error.message : String(error),
    problems: [],
    not_running: false,
  };
}

const live = {
  status: () => invoke<StatusReport>("status"),
  describe: () => invoke<Vocabulary>("describe"),
  bindings: () => invoke<{ bindings: BindingSpec[] }>("bindings"),
  bind: (spec: BindingSpec) => invoke<EditReport>("bind", { spec }),
  unbind: (address: BindingAddress) => invoke<EditReport>("unbind", { address }),
  test: (address: BindingAddress) => invoke<TestReport>("test", { address }),
  sessions: () => invoke<{ sessions: SessionInfo[] }>("sessions"),
  workspaces: () => invoke<{ workspaces: WorkspaceInfo[] }>("workspaces"),
  addPage: (spec: PageSpec) => invoke<EditReport>("add_page", { spec }),
  removePage: (page: string) => invoke<EditReport>("remove_page", { page }),
};

/** What Studio talks to. */
export type Client = typeof live;

let resolved: Client | null = null;

/**
 * The running PushOS, or sample data when Studio is being developed in a
 * browser.
 *
 * The sample data is loaded on demand, so a production build never carries it:
 * the condition is compile-time false and the import is never reached. Resolved
 * lazily rather than with a top-level await, so a module that fails to load
 * cannot stop the page from rendering an explanation.
 */
export async function client(): Promise<Client> {
  if (resolved === null) {
    resolved = usePreview() ? (await import("./preview")).preview : live;
  }
  return resolved;
}

/** How a surface should be described, and how urgently. */
export function describeSurface(surface: SurfaceReport): {
  text: string;
  tone: "good" | "idle" | "bad";
} {
  switch (surface) {
    case "push2":
      return { text: "Push 2 attached", tone: "good" };
    case "simulated":
      return { text: "Simulated surface — no hardware", tone: "bad" };
    case "absent":
      return { text: "No surface attached", tone: "idle" };
  }
}

/** The action namespace that drives a kind of session. */
export function providerFor(kind: SessionKind): string {
  return kind === "agent" ? "agent" : "terminal";
}

/**
 * Whether an action and a target belong together.
 *
 * A terminal target on an agent action is a binding that will always fail, and
 * saying so before it is written is cheaper than finding out on the hardware.
 */
export function targetSuitsAction(
  action: string,
  sessions: SessionInfo[],
  target: string,
): string | null {
  if (target === "" || action === "") return null;

  const session = sessions.find(
    (candidate) =>
      candidate.target === target || candidate.standing_target === target,
  );
  if (session === undefined) return null;

  const wanted = providerFor(session.kind);
  const provider = action.split(".")[0] ?? "";
  if (provider === wanted) return null;

  return `${session.name} is a ${session.kind}; ${wanted} actions drive it, not ${provider}.`;
}

/** Whether two addresses point at the same binding. */
export function sameAddress(a: BindingAddress, b: BindingAddress): boolean {
  return (
    a.control === b.control &&
    a.gesture === b.gesture &&
    (a.page ?? null) === (b.page ?? null) &&
    (a.workspace ?? null) === (b.workspace ?? null)
  );
}
