// The virtual Push 2.
//
// Every control PushOS reports is drawn. A control with a binding on the page
// being edited is marked, so the surface shows what the operator would actually
// find under their hands.

import type { BindingSpec, ControlInfo } from "../api";
import { captionFor, groupControls } from "../layout";

/** What the surface needs in order to draw itself. */
export interface SurfaceModel {
  controls: ControlInfo[];
  bindings: BindingSpec[];
  /** The page whose bindings are shown, or null for global only. */
  page: string | null;
  /** The control currently being edited. */
  selected: string | null;
}

/** Builds the virtual surface, calling back when a control is chosen. */
export function renderSurface(
  model: SurfaceModel,
  onSelect: (control: string) => void,
): HTMLElement {
  const surface = element("div", "surface");
  const bound = boundControls(model);
  const pads = model.controls.filter((control) => control.grid !== undefined);

  for (const group of groupControls(model.controls.map((control) => control.id))) {
    const section = element("section", "group");
    section.append(element("h2", "group-title", group.title));

    if (group.title === "Pads") {
      section.append(renderPads(pads, bound, model.selected, onSelect));
    } else {
      const row = element("div", `controls ${group.shape}`);
      for (const id of group.members) {
        row.append(renderControl(id, bound, model.selected, onSelect));
      }
      section.append(row);
    }

    surface.append(section);
  }

  return surface;
}

/** How many bindings each control has on the page being edited. */
function boundControls(model: SurfaceModel): Map<string, number> {
  const counts = new Map<string, number>();

  for (const binding of model.bindings) {
    // A global binding applies wherever you are, so it shows on every page.
    const applies = binding.page === undefined || binding.page === model.page;
    if (!applies) continue;
    counts.set(binding.control, (counts.get(binding.control) ?? 0) + 1);
  }

  return counts;
}

function renderPads(
  pads: ControlInfo[],
  bound: Map<string, number>,
  selected: string | null,
  onSelect: (control: string) => void,
): HTMLElement {
  const grid = element("div", "pad-grid");
  // Sorted by position rather than by name, so `pad.10` does not land between
  // `pad.1` and `pad.2`.
  const ordered = [...pads].sort((a, b) => {
    const first = a.grid;
    const second = b.grid;
    if (!first || !second) return 0;
    return first.row - second.row || first.column - second.column;
  });

  for (const pad of ordered) {
    grid.append(renderControl(pad.id, bound, selected, onSelect));
  }
  return grid;
}

function renderControl(
  id: string,
  bound: Map<string, number>,
  selected: string | null,
  onSelect: (control: string) => void,
): HTMLElement {
  const count = bound.get(id) ?? 0;
  const button = document.createElement("button");
  button.type = "button";
  button.className = "control";
  button.dataset.control = id;
  if (count > 0) button.classList.add("bound");
  if (id === selected) button.classList.add("selected");

  // The count is written out as well as shown, so the surface is readable
  // without relying on the highlight alone.
  button.title = count === 0 ? `${id} — not bound here` : `${id} — ${count} binding(s)`;
  button.setAttribute("aria-pressed", String(id === selected));

  button.append(element("span", "caption", captionFor(id)));
  if (count > 0) button.append(element("span", "count", String(count)));

  button.addEventListener("click", () => onSelect(id));
  return button;
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
