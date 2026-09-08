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

export interface StatusReport {
  protocol: number;
  version: string;
  push_connected: boolean;
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

export interface EditReport {
  file: string;
  replaced: boolean;
  binding_count: number;
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

/** Whether two addresses point at the same binding. */
export function sameAddress(a: BindingAddress, b: BindingAddress): boolean {
  return (
    a.control === b.control &&
    a.gesture === b.gesture &&
    (a.page ?? null) === (b.page ?? null) &&
    (a.workspace ?? null) === (b.workspace ?? null)
  );
}
