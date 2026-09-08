// Where each control actually sits on a Push 2.
//
// The surface is drawn as the instrument, not as a list of names grouped by
// kind, because the operator's hands know the instrument. A pad two rows down
// and three across should be two rows down and three across on screen, the
// touch strip should be to the left of the pads, and the encoders should be in
// a row above the display where they are.
//
// The set of controls still comes from PushOS, never from here. This only says
// where to draw them, and anything PushOS reports that this file does not place
// is drawn anyway, below the instrument, so a control cannot become invisible
// because the layout was not updated.

/** How a cluster of controls is drawn. */
export type ClusterKind =
  | "knob"
  | "bar"
  | "button"
  | "pads"
  | "strip"
  | "scenes"
  | "arrows"
  | "display";

/** One run of controls drawn the same way. */
export interface Cluster {
  kind: ClusterKind;
  members: string[];
  /** How many across. Ignored by clusters with a shape of their own. */
  columns?: number;
}

/** One region of the instrument. */
export interface Zone {
  /** The grid area it occupies. */
  area: string;
  /** What the region is, for anyone reading the page with a screen reader. */
  label: string;
  /** Whether its clusters stack or sit side by side. */
  flow: "row" | "column";
  clusters: Cluster[];
}

const upper = Array.from({ length: 8 }, (_, index) => `button.upper_${index + 1}`);
const lower = Array.from({ length: 8 }, (_, index) => `button.lower_${index + 1}`);
const trackEncoders = Array.from({ length: 8 }, (_, index) => `encoder.${index}`);

/**
 * The eight buttons to the right of the pads, coarsest at the top.
 *
 * That is the order the hardware numbers them in, and reversing it would put a
 * pad-adjacent control somewhere the operator's hand does not expect.
 */
const scenes = [
  "button.div_1_4",
  "button.div_1_4t",
  "button.div_1_8",
  "button.div_1_8t",
  "button.div_1_16",
  "button.div_1_16t",
  "button.div_1_32",
  "button.div_1_32t",
];

/** The instrument, region by region. */
export const ZONES: Zone[] = [
  {
    area: "tempo",
    label: "Tempo and swing",
    flow: "column",
    clusters: [
      { kind: "knob", members: ["encoder.tempo", "encoder.swing"] },
      {
        kind: "button",
        members: ["button.tap_tempo", "button.metronome"],
        columns: 2,
      },
    ],
  },
  {
    area: "knobs",
    label: "Encoders above the display",
    flow: "column",
    clusters: [
      { kind: "knob", members: trackEncoders },
      { kind: "bar", members: upper, columns: 8 },
    ],
  },
  {
    area: "master-knob",
    label: "Master and setup",
    flow: "column",
    clusters: [
      { kind: "knob", members: ["encoder.master"] },
      { kind: "button", members: ["button.setup", "button.user"], columns: 2 },
    ],
  },

  {
    area: "edit",
    label: "Edit",
    flow: "column",
    clusters: [
      { kind: "button", members: ["button.delete", "button.undo"], columns: 1 },
    ],
  },
  { area: "display", label: "Display", flow: "column", clusters: [{ kind: "display", members: [] }] },
  {
    area: "add",
    label: "Add and browse",
    flow: "row",
    clusters: [
      {
        kind: "button",
        members: ["button.add_device", "button.add_track"],
        columns: 1,
      },
      {
        kind: "button",
        members: ["button.device", "button.mix", "button.browse", "button.clip"],
        columns: 2,
      },
    ],
  },

  {
    area: "track-state",
    label: "Track state",
    flow: "column",
    clusters: [
      {
        kind: "button",
        members: ["button.mute", "button.solo", "button.stop"],
        columns: 3,
      },
    ],
  },
  { area: "columns", label: "Buttons below the display", flow: "column", clusters: [{ kind: "bar", members: lower, columns: 8 }] },
  {
    area: "navigate",
    label: "Master and navigation",
    flow: "row",
    clusters: [
      { kind: "button", members: ["button.master"], columns: 1 },
      {
        kind: "arrows",
        members: ["button.up", "button.left", "button.right", "button.down"],
      },
    ],
  },

  {
    area: "workflow",
    label: "Workflow and transport",
    flow: "column",
    clusters: [
      {
        kind: "button",
        members: [
          "button.convert",
          "button.double_loop",
          "button.quantize",
          "button.duplicate",
          "button.new",
          "button.fixed_length",
          "button.automate",
          "button.record",
          "button.play",
        ],
        columns: 1,
      },
    ],
  },
  { area: "strip", label: "Touch strip", flow: "column", clusters: [{ kind: "strip", members: ["touchstrip"] }] },
  { area: "pads", label: "Pads", flow: "column", clusters: [{ kind: "pads", members: [] }] },
  { area: "scenes", label: "Scene and note division buttons", flow: "column", clusters: [{ kind: "scenes", members: scenes }] },
  {
    area: "modes",
    label: "Modes and octave",
    flow: "column",
    clusters: [
      { kind: "button", members: ["button.repeat", "button.accent"], columns: 2 },
      { kind: "button", members: ["button.scale", "button.layout"], columns: 2 },
      { kind: "button", members: ["button.note", "button.session"], columns: 2 },
      { kind: "button", members: ["button.octave_up", "button.octave_down"], columns: 2 },
      { kind: "button", members: ["button.page_left", "button.page_right"], columns: 2 },
      { kind: "button", members: ["button.shift", "button.select"], columns: 2 },
    ],
  },
];

/** Every control this file places. */
export function placedControls(): Set<string> {
  return new Set(ZONES.flatMap((zone) => zone.clusters.flatMap((cluster) => cluster.members)));
}

/**
 * Controls PushOS reports that the instrument above does not place.
 *
 * Drawn separately rather than dropped: a control that exists but cannot be
 * seen cannot be bound, and the operator would have no way to know why.
 */
export function unplacedControls(ids: string[]): string[] {
  const placed = placedControls();
  return ids.filter((id) => !placed.has(id) && !id.startsWith("pad."));
}

/** The short caption drawn on a control. */
export function captionFor(id: string): string {
  if (id === "touchstrip") return "Touch strip";

  const [namespace, ...rest] = id.split(".");
  const name = rest.join(".");

  if (namespace === "pad") return name;
  if (namespace === "encoder") {
    const position = Number(name);
    return Number.isNaN(position) ? title(name) : String(position + 1);
  }

  if (name.startsWith("upper_") || name.startsWith("lower_")) {
    return name.split("_")[1] ?? name;
  }
  if (name.startsWith("div_")) {
    return name.slice(4).replace("_", "/");
  }
  return title(name);
}

/** The symbol a control carries instead of a word, when it has one. */
export function symbolFor(id: string): string | null {
  const symbols: Record<string, string> = {
    "button.play": "▶",
    "button.record": "●",
    "button.up": "∧",
    "button.down": "∨",
    "button.left": "‹",
    "button.right": "›",
    "button.octave_up": "Oct ∧",
    "button.octave_down": "Oct ∨",
    "button.page_left": "◀",
    "button.page_right": "▶",
  };
  return symbols[id] ?? null;
}

function title(name: string): string {
  return name
    .split("_")
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(" ");
}
