// Sample data for working on Studio's appearance in a browser.
//
// Only ever reachable from a development build with no Tauri bridge present,
// which is exactly the case when someone runs `npm run dev` to iterate on the
// layout. A production build cannot take this path: the check is on
// `import.meta.env.DEV`, which the bundler resolves to `false` and removes.
//
// The banner is not optional. Sample data that looks like real data is worse
// than no data at all.

import type {
  BindingSpec,
  EditReport,
  StatusReport,
  TestReport,
  Vocabulary,
} from "./api";

const controls: Vocabulary["controls"] = [
  ...Array.from({ length: 64 }, (_, index) => ({
    id: `pad.${index}`,
    kind: "pad" as const,
    illuminated: true,
    grid: { row: Math.floor(index / 8), column: index % 8 },
  })),
  ...[
    "play",
    "record",
    "automate",
    "fixed_length",
    "new",
    "duplicate",
    "quantize",
    "double_loop",
    "metronome",
    "tap_tempo",
    "delete",
    "undo",
    "up",
    "down",
    "left",
    "right",
    "page_left",
    "page_right",
    "octave_up",
    "octave_down",
    "shift",
    "select",
    "session",
    "note",
    "layout",
    "scale",
    "accent",
    "repeat",
    "setup",
    "user",
    "device",
    "browse",
    "mix",
    "clip",
    "add_device",
    "add_track",
    "master",
    "mute",
    "solo",
    "stop",
    "convert",
    "div_1_32t",
    "div_1_32",
    "div_1_16t",
    "div_1_16",
    "div_1_8t",
    "div_1_8",
    "div_1_4t",
    "div_1_4",
    ...Array.from({ length: 8 }, (_, index) => `upper_${index + 1}`),
    ...Array.from({ length: 8 }, (_, index) => `lower_${index + 1}`),
  ].map((name) => ({ id: `button.${name}`, kind: "button" as const, illuminated: true })),
  ...["tempo", "swing", "0", "1", "2", "3", "4", "5", "6", "7", "master"].map((name) => ({
    id: `encoder.${name}`,
    kind: "encoder" as const,
    illuminated: false,
  })),
  { id: "touchstrip", kind: "touch_strip" as const, illuminated: false },
];

const vocabulary: Vocabulary = {
  controls,
  gestures: [
    "press",
    "release",
    "tap",
    "double_tap",
    "hold",
    "shift_press",
    "shift_hold",
    "turn",
    "turn_left",
    "turn_right",
    "shift_turn",
    "touch",
    "touch_release",
  ],
  pages: [
    { id: "home", name: "Home", description: "Transport and the things you reach for" },
    { id: "development", name: "Development", description: "Projects, editors and test runs" },
    { id: "music", name: "Music", description: "Playback and volume" },
  ],
  providers: [
    { name: "app", verbs: ["launch", "focus", "open_path", "open_url"], requires: ["application.launch"], permitted: true },
    { name: "media", verbs: ["play_pause", "next_track", "previous_track", "volume_up", "volume_down", "set_volume", "now_playing"], requires: ["media.control"], permitted: true },
    { name: "page", verbs: ["show", "next", "previous", "home", "back"], requires: [], permitted: true },
    { name: "shell", verbs: ["run"], requires: ["shell.execute"], permitted: false },
    { name: "shortcut", verbs: ["run", "list"], requires: ["shortcuts.execute"], permitted: true },
  ],
};

const bindings: BindingSpec[] = [
  { control: "button.play", gesture: "press", action: "media.play_pause", label: "Play/Pause" },
  { control: "button.page_right", gesture: "press", action: "page.next", label: "Next page" },
  { control: "button.page_left", gesture: "press", action: "page.previous", label: "Previous page" },
  { control: "button.session", gesture: "press", action: "page.home", label: "Home" },
  { control: "button.upper_1", gesture: "press", action: "page.show", target: "home", label: "Home" },
  { control: "button.upper_2", gesture: "press", action: "page.show", target: "development", label: "Develop" },
  { control: "button.upper_3", gesture: "press", action: "page.show", target: "music", label: "Music" },
  { control: "button.lower_1", gesture: "press", action: "media.play_pause", label: "Play/Pause" },
  { control: "pad.0", gesture: "tap", page: "development", action: "app.launch", target: "Cursor", label: "Cursor" },
  { control: "pad.0", gesture: "hold", page: "development", action: "app.open_url", target: "https://github.com", label: "Repository" },
  { control: "pad.1", gesture: "tap", page: "development", action: "app.launch", target: "Terminal", label: "Terminal" },
  { control: "pad.9", gesture: "tap", page: "development", action: "shell.run", label: "Tests" },
  { control: "pad.0", gesture: "tap", page: "music", action: "media.play_pause", label: "Play/Pause" },
];

const status: StatusReport = {
  protocol: 1,
  version: "0.1.0 (sample data)",
  push_connected: true,
  page: "development",
  workspace: null,
  config_root: "~/.config/pushos",
  binding_count: bindings.length,
};

const refused = () => {
  throw {
    message: "Studio is showing sample data; there is nothing to change.",
    problems: [],
    not_running: false,
  };
};

/** The same shape as the real client, backed by sample data. */
export const preview = {
  status: (): Promise<StatusReport> => Promise.resolve(status),
  describe: (): Promise<Vocabulary> => Promise.resolve(vocabulary),
  bindings: (): Promise<{ bindings: BindingSpec[] }> => Promise.resolve({ bindings }),
  bind: (): Promise<EditReport> => refused(),
  unbind: (): Promise<EditReport> => refused(),
  test: (): Promise<TestReport> => refused(),
};
