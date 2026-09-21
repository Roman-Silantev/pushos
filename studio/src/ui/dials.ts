// The panel for the settings that are not bindings.
//
// How many sessions may run, how bright the panel is, how long a press has to
// last to be a hold. These are the things an operator wants to try, look at,
// and try again, which is exactly what a text file is bad at.
//
// A dial left empty is not zero. Empty means "I have not said", and PushOS
// uses its default — which the box says, so nobody has to guess. Clearing a
// box is how a dial goes back to being unset.

import type { Settings, SettingsDefaults, SettingsReport } from "../api";
import { element } from "./surface";

/** One dial, and how to read and write it on the settings. */
interface Dial {
  /** What it is called on screen. */
  name: string;
  /** What it means, in a sentence an operator can act on. */
  detail: string;
  /** What PushOS does when nobody has set it. */
  fallback: (defaults: SettingsDefaults) => string;
  /** What it is set to now, if it is set at all. */
  read: (settings: Settings) => number | undefined;
  /** Sets it, or unsets it when given nothing. */
  write: (settings: Settings, value: number | undefined) => void;
  /** The smallest value worth offering. */
  least?: number;
  /** The largest, where the hardware or the Mac has one. */
  most?: number;
  /** How finely it can be adjusted. */
  step?: number;
}

/** What each group of dials is for. */
interface Group {
  title: string;
  detail: string;
  dials: Dial[];
}

/** Every dial, grouped the way an operator thinks about them. */
export const GROUPS: Group[] = [
  {
    title: "The fleet",
    detail:
      "What the sessions on your pads are allowed to cost. Memory is one limit; the model answering more than a handful of streams at once is the other.",
    dials: [
      {
        name: "Sessions running at once",
        detail:
          "Each holds its memory whether or not it is working. Past this many, the ones idle longest are put away, keeping their conversations. 0 means no limit.",
        fallback: (defaults) => describe(defaults.most_live),
        read: (settings) => settings.sessions.most_live,
        write: (settings, value) => {
          put(settings.sessions, "most_live", value);
        },
        least: 0,
        most: 64,
      },
      {
        name: "Codex threads loaded",
        detail:
          "Threads share one server and cost a fraction of a session each, so this can be one per pad. 0 means no limit.",
        fallback: (defaults) => describe(defaults.most_threads),
        read: (settings) => settings.sessions.most_threads,
        write: (settings, value) => {
          put(settings.sessions, "most_threads", value);
        },
        least: 0,
        most: 64,
      },
      {
        name: "Working at once",
        detail:
          "Not a memory limit. A model answering too many streams at once replies with refusals, and a refusal loses the whole turn, so work given past this waits and goes as soon as one finishes. 0 starts everything at once.",
        fallback: (defaults) => describe(defaults.working_at_once),
        read: (settings) => settings.sessions.working_at_once,
        write: (settings, value) => {
          put(settings.sessions, "working_at_once", value);
        },
        least: 0,
        most: 64,
      },
      {
        name: "Put away after (minutes)",
        detail:
          "A pad nobody has used since this morning need not hold half a gigabyte. 0 keeps idle sessions running until the limit says otherwise.",
        fallback: (defaults) => describe(defaults.put_away_after_minutes),
        read: (settings) => settings.sessions.put_away_after_minutes,
        write: (settings, value) => {
          put(settings.sessions, "put_away_after_minutes", value);
        },
        least: 0,
        most: 600,
      },
    ],
  },
  {
    title: "The panel",
    detail:
      "How hard the hardware works, and when it rests. Every LED is lit the whole time PushOS is running.",
    dials: [
      {
        name: "Brightness (%)",
        detail:
          "Bright enough to read across a desk. Lower takes the hardest third off every LED for the whole of the day.",
        fallback: (defaults) => `${defaults.brightness}`,
        read: (settings) => settings.surface.brightness,
        write: (settings, value) => {
          put(settings.surface, "brightness", value);
        },
        least: 1,
        most: 100,
      },
      {
        name: "Dim after (minutes)",
        detail: "Untouched for this long and the panel dims. 0 never dims.",
        fallback: (defaults) => describe(defaults.dim_after_minutes),
        read: (settings) => settings.surface.dim_after_minutes,
        write: (settings, value) => {
          put(settings.surface, "dim_after_minutes", value);
        },
        least: 0,
        step: 0.5,
      },
      {
        name: "Dark after (minutes)",
        detail:
          "Untouched for this long and the screen goes dark altogether. 0 never does.",
        fallback: (defaults) => describe(defaults.sleep_after_minutes),
        read: (settings) => settings.surface.sleep_after_minutes,
        write: (settings, value) => {
          put(settings.surface, "sleep_after_minutes", value);
        },
        least: 0,
        step: 0.5,
      },
    ],
  },
  {
    title: "The feel",
    detail:
      "What separates a tap from a hold under your own hand. Worth adjusting once you have used the surface for an hour.",
    dials: [
      {
        name: "Hold after (ms)",
        detail: "A press held at least this long is a hold rather than a tap.",
        fallback: (defaults) => `${defaults.hold_threshold_ms}`,
        read: (settings) => settings.gestures.hold_threshold_ms,
        write: (settings, value) => {
          put(settings.gestures, "hold_threshold_ms", value);
        },
        least: 50,
        most: 2000,
        step: 10,
      },
      {
        name: "Double tap within (ms)",
        detail: "A second press this soon after the first is a double tap.",
        fallback: (defaults) => `${defaults.double_tap_window_ms}`,
        read: (settings) => settings.gestures.double_tap_window_ms,
        write: (settings, value) => {
          put(settings.gestures, "double_tap_window_ms", value);
        },
        least: 50,
        most: 2000,
        step: 10,
      },
    ],
  },
];

/**
 * Sets a dial, or removes it when nobody has said what it should be.
 *
 * Removing rather than writing `undefined`: the key going away is what makes
 * the configuration say nothing about it, which is the whole distinction
 * between an unset dial and one set to a number that happens to be today's
 * default.
 */
function put<T extends object, K extends keyof T>(
  held: T,
  key: K,
  value: T[K] | undefined,
): void {
  if (value === undefined) {
    delete held[key];
    return;
  }
  held[key] = value;
}

/** How a default reads when PushOS has none, which means no limit at all. */
function describe(fallback: number | undefined): string {
  return fallback === undefined ? "no limit" : `${fallback}`;
}

/** What the panel can ask the application to do. */
export interface DialActions {
  /** Writes every dial, and reports what came back. */
  save: (settings: Settings) => void;
}

/**
 * Draws the panel.
 *
 * The settings are copied before they are edited, so a failed save leaves what
 * is on screen exactly as the operator left it rather than half-applied.
 */
export function renderDials(
  report: SettingsReport,
  actions: DialActions,
): HTMLElement {
  const panel = element("section", "dials");
  panel.append(element("h2", "dials-title", "Settings"));
  panel.append(
    element(
      "p",
      "dials-detail",
      "These are written to your configuration file, comments and all. Leave a box empty to let PushOS decide.",
    ),
  );

  const edited: Settings = structuredClone(report.settings);

  for (const group of GROUPS) {
    panel.append(renderGroup(group, edited, report.defaults));
  }

  const save = element("button", "dials-save", "Apply") as HTMLButtonElement;
  save.type = "button";
  save.addEventListener("click", () => actions.save(edited));
  panel.append(save);

  if (report.file !== undefined) {
    panel.append(element("p", "dials-file", `Written to ${report.file}`));
  }
  return panel;
}

/** One group of dials, under what it is for. */
function renderGroup(
  group: Group,
  edited: Settings,
  defaults: SettingsDefaults,
): HTMLElement {
  const section = element("section", "dial-group");
  section.append(element("h3", "dial-group-title", group.title));
  section.append(element("p", "dial-group-detail", group.detail));
  for (const dial of group.dials) {
    section.append(renderDial(dial, edited, defaults));
  }
  return section;
}

/** One dial, with what it does and what happens if it is left alone. */
function renderDial(
  dial: Dial,
  edited: Settings,
  defaults: SettingsDefaults,
): HTMLElement {
  const row = element("label", "dial");
  row.append(element("span", "dial-name", dial.name));

  const input = element("input", "dial-input") as HTMLInputElement;
  input.type = "number";
  if (dial.least !== undefined) input.min = `${dial.least}`;
  if (dial.most !== undefined) input.max = `${dial.most}`;
  input.step = `${dial.step ?? 1}`;
  input.placeholder = `${dial.fallback(defaults)} (default)`;

  const set = dial.read(edited);
  input.value = set === undefined ? "" : `${set}`;

  input.addEventListener("input", () => {
    // An empty box is a dial nobody has set, which is not the same as zero:
    // zero means "no limit" on several of these, and writing it because a box
    // was cleared would be the opposite of what was asked.
    const written = input.value.trim();
    dial.write(edited, written === "" ? undefined : Number(written));
  });

  row.append(input);
  row.append(element("span", "dial-detail", dial.detail));
  return row;
}
