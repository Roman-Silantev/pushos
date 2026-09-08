// The virtual Push 2.
//
// Drawn as the instrument rather than as a list of control names, because that
// is how it is used: the operator reaches for the pad two rows down and three
// across, not for `pad.19`. Every control PushOS reports is drawn, and one with
// a binding on the page being edited is marked, so the surface shows what would
// actually be under their hands.

import type { BindingSpec, ControlInfo } from "../api";
import type { Cluster, Zone } from "../layout";
import { ZONES, captionFor, symbolFor, unplacedControls } from "../layout";

/** What the surface needs in order to draw itself. */
export interface SurfaceModel {
  controls: ControlInfo[];
  bindings: BindingSpec[];
  /** The page whose bindings are shown, or null for global only. */
  page: string | null;
  /** The project whose bindings are shown, or null for every project. */
  workspace: string | null;
  /** The control currently being edited. */
  selected: string | null;
}

/** How many columns the display and the button rows either side of it have. */
const COLUMNS = 8;

/** Builds the virtual surface, calling back when a control is chosen. */
export function renderSurface(
  model: SurfaceModel,
  onSelect: (control: string) => void,
): HTMLElement {
  const surface = element("div", "surface");
  const context: Context = {
    bound: boundControls(model),
    labels: labelsByControl(model),
    pads: padsInOrder(model.controls),
    available: new Set(model.controls.map((control) => control.id)),
    selected: model.selected,
    page: model.page,
    workspace: model.workspace,
    onSelect,
  };

  const device = element("div", "push");
  for (const zone of ZONES) {
    device.append(renderZone(zone, context));
  }
  surface.append(device);

  // Anything the layout does not place is still bindable, and still shown.
  const stray = unplacedControls(model.controls.map((control) => control.id));
  if (stray.length > 0) {
    const extra = element("section", "unplaced");
    extra.append(element("h2", "group-title", "Not on the diagram"));
    const row = element("div", "unplaced-row");
    for (const id of stray) row.append(renderButton(id, context));
    extra.append(row);
    surface.append(extra);
  }

  return surface;
}

/** Everything the drawing needs, gathered once. */
interface Context {
  bound: Map<string, number>;
  labels: Map<string, string>;
  pads: ControlInfo[];
  available: Set<string>;
  selected: string | null;
  page: string | null;
  workspace: string | null;
  onSelect: (control: string) => void;
}

function renderZone(zone: Zone, context: Context): HTMLElement {
  const region = element("div", `zone zone-${zone.area} flow-${zone.flow}`);
  region.style.gridArea = zone.area;
  region.setAttribute("aria-label", zone.label);

  for (const cluster of zone.clusters) {
    region.append(renderCluster(cluster, context));
  }
  return region;
}

function renderCluster(cluster: Cluster, context: Context): HTMLElement {
  switch (cluster.kind) {
    case "pads":
      return renderPads(context);
    case "display":
      return renderDisplay(context);
    case "strip":
      return renderStrip(context);
    case "arrows":
      return renderArrows(cluster, context);
    case "scenes":
      return renderColumn("scenes", cluster, context);
    case "knob":
      return renderKnobs(cluster, context);
    case "bar":
      return renderColumn("bars", cluster, context);
    case "button":
      return renderColumn("buttons", cluster, context);
  }
}

/** A run of controls laid out in a fixed number of columns. */
function renderColumn(
  kind: string,
  cluster: Cluster,
  context: Context,
): HTMLElement {
  const group = element("div", `cluster ${kind}`);
  group.style.setProperty("--columns", String(cluster.columns ?? 1));
  for (const id of cluster.members) {
    group.append(renderButton(id, context));
  }
  return group;
}

function renderKnobs(cluster: Cluster, context: Context): HTMLElement {
  const group = element("div", "cluster knobs");
  group.style.setProperty("--columns", String(cluster.members.length));

  for (const id of cluster.members) {
    const knob = shell(id, "knob", context);
    knob.append(element("span", "knob-face"));
    knob.append(element("span", "knob-caption", captionFor(id)));
    group.append(knob);
  }
  return group;
}

/**
 * The touch strip.
 *
 * Real and bindable, not decoration: it reports touch and release like any
 * other control, and a binding can use it.
 */
function renderStrip(context: Context): HTMLElement {
  const group = element("div", "cluster strip");
  const strip = shell("touchstrip", "touch", context);
  strip.append(element("span", "touch-track"));
  strip.append(element("span", "touch-caption", "Touch"));
  group.append(strip);
  return group;
}

/** The four-way cursor, drawn as the diamond it is. */
function renderArrows(cluster: Cluster, context: Context): HTMLElement {
  const group = element("div", "cluster arrows");
  for (const id of cluster.members) {
    const button = renderButton(id, context);
    // Placed by name rather than by order, so the diamond survives an edit to
    // the layout that reorders the list.
    button.classList.add(`arrow-${id.split(".")[1] ?? ""}`);
    group.append(button);
  }
  return group;
}

function renderPads(context: Context): HTMLElement {
  const grid = element("div", "cluster pad-grid");
  for (const pad of context.pads) {
    grid.append(renderButton(pad.id, context));
  }
  return grid;
}

/**
 * The display, showing what the hardware would show.
 *
 * The eight columns line up with the eight buttons above and below it, so the
 * captions on those bindings are exactly what appears here.
 */
function renderDisplay(context: Context): HTMLElement {
  const screen = element("div", "cluster screen");
  const header = element("div", "screen-header");
  header.append(
    element("span", "screen-page", context.page === null ? "Everywhere" : context.page),
  );
  screen.append(header);

  const columns = element("div", "screen-columns");
  for (let index = 1; index <= COLUMNS; index += 1) {
    const column = element("div", "screen-column");
    const above = context.labels.get(`button.upper_${index}`);
    const below = context.labels.get(`button.lower_${index}`);

    if (above === undefined && below === undefined) {
      column.append(element("span", "screen-empty", "—"));
    } else {
      if (above !== undefined) column.append(element("span", "screen-top", above));
      if (below !== undefined) column.append(element("span", "screen-bottom", below));
    }
    columns.append(column);
  }
  screen.append(columns);
  return screen;
}

/** A control drawn as a labelled key. */
function renderButton(id: string, context: Context): HTMLElement {
  const button = shell(id, "key", context);
  const symbol = symbolFor(id);
  button.append(element("span", "caption", symbol ?? captionFor(id)));
  if (symbol !== null) button.classList.add("symbolic");

  const label = context.labels.get(id);
  if (label !== undefined) button.append(element("span", "assigned", label));
  return button;
}

/**
 * The shared parts of every control: what it is, whether it is bound, whether
 * it is chosen, and what happens when it is clicked.
 */
function shell(id: string, kind: string, context: Context): HTMLElement {
  const count = context.bound.get(id) ?? 0;
  const node = document.createElement("button");
  node.type = "button";
  node.className = `control ${kind}`;
  node.dataset.control = id;

  if (count > 0) node.classList.add("bound");
  if (id === context.selected) node.classList.add("selected");
  // A control PushOS does not report is drawn in place, faintly, rather than
  // left as a hole in the instrument.
  if (!context.available.has(id)) node.classList.add("absent");

  // Written out as well as shown, so the surface is readable without relying
  // on the highlight alone.
  node.title = count === 0 ? `${id} — not bound here` : `${id} — ${count} binding(s)`;
  node.setAttribute("aria-pressed", String(id === context.selected));

  if (count > 0) node.append(element("span", "count", String(count)));
  node.addEventListener("click", () => context.onSelect(id));
  return node;
}

/** How many bindings each control has on the page being edited. */
function boundControls(model: SurfaceModel): Map<string, number> {
  const counts = new Map<string, number>();

  for (const binding of model.bindings) {
    if (!applies(binding, model)) continue;
    counts.set(binding.control, (counts.get(binding.control) ?? 0) + 1);
  }

  return counts;
}

/** The caption each control carries, taken from the binding that set one. */
function labelsByControl(model: SurfaceModel): Map<string, string> {
  const labels = new Map<string, string>();

  for (const binding of model.bindings) {
    if (!applies(binding, model) || binding.label === undefined) continue;
    // The narrower caption wins over one that applies more widely, which is
    // what the hardware shows.
    if (binding.page !== undefined || !labels.has(binding.control)) {
      labels.set(binding.control, binding.label);
    }
  }

  return labels;
}

/**
 * Whether a binding is in force where the operator is looking.
 *
 * A binding that applies everywhere shows on every page and in every project,
 * which is what "everywhere" means.
 */
function applies(binding: BindingSpec, model: SurfaceModel): boolean {
  const onPage = binding.page === undefined || binding.page === model.page;
  const inProject =
    binding.workspace === undefined || binding.workspace === model.workspace;
  return onPage && inProject;
}

/** Pads in hardware order, so `pad.10` does not land between `pad.1` and `pad.2`. */
function padsInOrder(controls: ControlInfo[]): ControlInfo[] {
  return controls
    .filter((control) => control.grid !== undefined)
    .sort((a, b) => {
      const first = a.grid;
      const second = b.grid;
      if (!first || !second) return 0;
      return first.row - second.row || first.column - second.column;
    });
}

/** Creates an element with a class and optional text. */
export function element(
  tag: string,
  className: string,
  text?: string,
): HTMLElement {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}
