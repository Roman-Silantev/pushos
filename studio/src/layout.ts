// Where each control sits on the virtual Push 2.
//
// The set of controls comes from PushOS, never from here: this only says where
// to draw them. A control PushOS reports that this file does not place still
// appears, in a group of its own, so nothing can become invisible just because
// the layout was not updated.

export interface ControlGroup {
  /** Heading shown above the group. */
  title: string;
  /** Control ids, in the order they are drawn. */
  members: string[];
  /** How the group is arranged. */
  shape: "row" | "grid" | "column";
}

const upper = Array.from({ length: 8 }, (_, index) => `button.upper_${index + 1}`);
const lower = Array.from({ length: 8 }, (_, index) => `button.lower_${index + 1}`);
const trackEncoders = Array.from({ length: 8 }, (_, index) => `encoder.${index}`);

/** The groups, in the order they appear down the page. */
export const GROUPS: ControlGroup[] = [
  {
    title: "Encoders",
    shape: "row",
    members: ["encoder.tempo", "encoder.swing", ...trackEncoders, "encoder.master"],
  },
  { title: "Above the display", shape: "row", members: upper },
  { title: "Below the display", shape: "row", members: lower },
  {
    title: "Pads",
    shape: "grid",
    // Filled in from the vocabulary, since pads carry their own grid position.
    members: [],
  },
  {
    title: "Note divisions",
    shape: "column",
    members: [
      "button.div_1_32t",
      "button.div_1_32",
      "button.div_1_16t",
      "button.div_1_16",
      "button.div_1_8t",
      "button.div_1_8",
      "button.div_1_4t",
      "button.div_1_4",
    ],
  },
  {
    title: "Transport",
    shape: "row",
    members: [
      "button.play",
      "button.record",
      "button.automate",
      "button.fixed_length",
      "button.new",
      "button.duplicate",
      "button.quantize",
      "button.double_loop",
      "button.metronome",
      "button.tap_tempo",
      "button.delete",
      "button.undo",
    ],
  },
  {
    title: "Navigation",
    shape: "row",
    members: [
      "button.up",
      "button.down",
      "button.left",
      "button.right",
      "button.page_left",
      "button.page_right",
      "button.octave_up",
      "button.octave_down",
      "button.shift",
      "button.select",
    ],
  },
  {
    title: "Modes",
    shape: "row",
    members: [
      "button.session",
      "button.note",
      "button.layout",
      "button.scale",
      "button.accent",
      "button.repeat",
      "button.setup",
      "button.user",
      "button.device",
      "button.browse",
      "button.mix",
      "button.clip",
      "button.add_device",
      "button.add_track",
      "button.master",
      "button.mute",
      "button.solo",
      "button.stop",
      "button.convert",
    ],
  },
];

/** The short caption drawn on a control. */
export function captionFor(id: string): string {
  if (id === "touchstrip") return "Touch";

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

function title(name: string): string {
  return name
    .split("_")
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(" ");
}

/**
 * Splits every control into its group, putting anything this file does not
 * place into a final group rather than dropping it.
 */
export function groupControls(ids: string[]): ControlGroup[] {
  const placed = new Set(GROUPS.flatMap((group) => group.members));
  const available = new Set(ids);

  const groups = GROUPS.map((group) => ({
    ...group,
    members: group.members.filter((id) => available.has(id)),
  }));

  const unplaced = ids.filter((id) => !placed.has(id) && !id.startsWith("pad."));
  if (unplaced.length > 0) {
    groups.push({ title: "Other", shape: "row", members: unplaced });
  }

  return groups.filter((group) => group.members.length > 0 || group.title === "Pads");
}
