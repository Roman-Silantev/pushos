// The panel for the selected control.
//
// Shows every gesture the control can carry and what each one currently does,
// so the answer to "what does this pad do" is one click away rather than a
// search through a file.

import type {
  BindingAddress,
  BindingSpec,
  ProviderInfo,
  SessionInfo,
  Vocabulary,
} from "../api";
import { targetSuitsAction } from "../api";
import { captionFor } from "../layout";
import { element } from "./surface";

/** What the inspector needs in order to draw itself. */
export interface InspectorModel {
  control: string | null;
  page: string | null;
  vocabulary: Vocabulary;
  bindings: BindingSpec[];
  /** What is running now, so a control can be pointed at one of them. */
  sessions: SessionInfo[];
}

/** What the inspector can ask the application to do. */
export interface InspectorActions {
  save: (spec: BindingSpec) => void;
  remove: (address: BindingAddress) => void;
  test: (address: BindingAddress) => void;
}

/** The gestures offered for a control, given what kind of control it is. */
export function gesturesFor(control: string, all: string[]): string[] {
  const turning = ["turn", "turn_left", "turn_right", "shift_turn"];
  const touching = ["touch", "touch_release"];
  const pressing = [
    "press",
    "release",
    "tap",
    "double_tap",
    "hold",
    "shift_press",
    "shift_hold",
  ];

  const wanted = (() => {
    if (control.startsWith("encoder.")) return [...turning, ...touching];
    if (control === "touchstrip") return touching;
    return pressing;
  })();

  // Intersected with what PushOS reports, so a gesture this file believes in
  // but the runtime does not is never offered.
  return wanted.filter((gesture) => all.includes(gesture));
}

/** Builds the inspector. */
export function renderInspector(
  model: InspectorModel,
  actions: InspectorActions,
): HTMLElement {
  const panel = element("div", "inspector");

  if (model.control === null) {
    panel.append(
      element("p", "empty", "Choose a control above to see and change what it does."),
    );
    return panel;
  }

  const header = element("div", "inspector-header");
  header.append(element("h2", "inspector-title", model.control));
  header.append(
    element(
      "p",
      "inspector-scope",
      model.page === null
        ? "Editing bindings that apply everywhere"
        : `Editing bindings on the ${pageName(model, model.page)} page`,
    ),
  );
  panel.append(header);

  for (const gesture of gesturesFor(model.control, model.vocabulary.gestures)) {
    panel.append(renderGesture(model, actions, model.control, gesture));
  }

  return panel;
}

function pageName(model: InspectorModel, id: string): string {
  return model.vocabulary.pages.find((page) => page.id === id)?.name ?? id;
}

function renderGesture(
  model: InspectorModel,
  actions: InspectorActions,
  control: string,
  gesture: string,
): HTMLElement {
  const address: BindingAddress = {
    control,
    gesture,
    ...(model.page === null ? {} : { page: model.page }),
  };
  const existing = model.bindings.find(
    (binding) =>
      binding.control === control &&
      binding.gesture === gesture &&
      (binding.page ?? null) === model.page,
  );

  const row = element("div", "gesture");
  if (existing) row.classList.add("assigned");

  const heading = element("div", "gesture-heading");
  heading.append(element("span", "gesture-name", readable(gesture)));
  heading.append(
    element("span", "gesture-state", existing ? existing.action : "not assigned"),
  );
  row.append(heading);

  const form = document.createElement("form");
  form.className = "gesture-form";

  const action = actionPicker(model.vocabulary.providers, existing?.action);
  const target = targetField(model.sessions, existing?.target ?? "");
  const label = textField("label", "Caption on the display", existing?.label ?? "");

  form.append(action.field);
  if (target.assign !== null) form.append(target.assign);
  form.append(target.field, label.field);

  const buttons = element("div", "gesture-buttons");
  const save = button("Save", "primary");
  buttons.append(save);

  if (existing) {
    const test = button("Test", "");
    test.addEventListener("click", (event) => {
      event.preventDefault();
      actions.test(address);
    });

    const remove = button("Remove", "danger");
    remove.addEventListener("click", (event) => {
      event.preventDefault();
      actions.remove(address);
    });

    buttons.append(test, remove);
  }
  form.append(buttons);

  // Said before the binding is written rather than discovered on the hardware.
  const mismatch = element("p", "field-warning");
  mismatch.hidden = true;
  const check = (): void => {
    const problem = targetSuitsAction(action.value(), model.sessions, target.value());
    mismatch.textContent = problem ?? "";
    mismatch.hidden = problem === null;
  };
  action.onChange(check);
  target.onChange(check);
  check();
  form.append(mismatch);

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const chosen = action.value();
    if (chosen === "") return;

    actions.save({
      ...address,
      action: chosen,
      ...(target.value() === "" ? {} : { target: target.value() }),
      ...(label.value() === "" ? {} : { label: label.value() }),
    });
  });

  row.append(form);
  return row;
}

/**
 * A picker listing exactly the actions the running PushOS has.
 *
 * A provider whose capability is not granted is still offered, marked, because
 * hiding it would leave no explanation for why the action is missing.
 */
function actionPicker(
  providers: ProviderInfo[],
  current: string | undefined,
): Field {
  const field = element("label", "field");
  field.append(element("span", "field-name", "Action"));

  const select = document.createElement("select");
  select.className = "field-input";

  const none = document.createElement("option");
  none.value = "";
  none.textContent = "Choose an action";
  select.append(none);

  for (const provider of providers) {
    const group = document.createElement("optgroup");
    group.label = provider.permitted
      ? provider.name
      : `${provider.name} (needs ${provider.requires.join(", ")})`;

    for (const verb of provider.verbs) {
      const option = document.createElement("option");
      option.value = `${provider.name}.${verb}`;
      option.textContent = `${provider.name}.${verb}`;
      option.selected = option.value === current;
      group.append(option);
    }
    select.append(group);
  }

  field.append(select);
  return {
    field,
    value: () => select.value,
    onChange: (run) => select.addEventListener("change", run),
  };
}

/**
 * The target, chosen from what is running or written by hand.
 *
 * Both forms are offered for every session because they mean different things:
 * naming a role or a terminal keeps working after this session ends, and naming
 * the session itself does not. The text stays editable so a target for
 * something that is not running yet can still be written.
 */
function targetField(sessions: SessionInfo[], initial: string): TargetField {
  const field = element("label", "field");
  field.append(element("span", "field-name", "Target"));

  const input = document.createElement("input");
  input.className = "field-input";
  input.name = "target";
  input.type = "text";
  input.value = initial;
  input.autocomplete = "off";
  input.placeholder = "Nothing in particular";
  field.append(input);

  const listeners: Array<() => void> = [];
  const announce = (): void => {
    for (const run of listeners) run();
  };
  input.addEventListener("input", announce);

  return {
    field,
    assign: sessions.length === 0 ? null : assignField(sessions, input, announce),
    value: () => input.value.trim(),
    onChange: (run) => listeners.push(run),
  };
}

/**
 * Picks a running session and writes it into the target beside it.
 *
 * Both forms are offered for every session because they mean different things:
 * naming a role or a terminal keeps working after this session ends, and naming
 * the session itself does not. The text stays editable, so a target for
 * something that is not running yet can still be written by hand.
 */
function assignField(
  sessions: SessionInfo[],
  input: HTMLInputElement,
  announce: () => void,
): HTMLElement {
  const field = element("label", "field");
  field.append(element("span", "field-name", "Assign a session"));

  const chooser = document.createElement("select");
  chooser.className = "field-input";

  const none = document.createElement("option");
  none.value = "";
  none.textContent = "Running now\u2026";
  chooser.append(none);

  for (const session of sessions) {
    const group = document.createElement("optgroup");
    const state = session.live ? session.status : `${session.status}, ended`;
    group.label = `${session.name} \u2014 ${session.kind}, ${state}`;

    if (session.standing_target !== undefined) {
      const standing = document.createElement("option");
      standing.value = session.standing_target;
      const holds = session.kind === "agent" ? "fills the role" : "holds the name";
      standing.textContent = `${session.standing_target} (whichever ${session.kind} ${holds})`;
      group.append(standing);
    }

    const exact = document.createElement("option");
    exact.value = session.target;
    exact.textContent = `${session.target} (this session only)`;
    group.append(exact);

    chooser.append(group);
  }

  chooser.addEventListener("change", () => {
    if (chooser.value === "") return;
    input.value = chooser.value;
    // Reset, so the picker reads as something done rather than as state that
    // could disagree with the text beside it.
    chooser.value = "";
    announce();
  });

  field.append(chooser);
  return field;
}

/** A form control the caller can read and watch. */
interface Field {
  field: HTMLElement;
  value: () => string;
  onChange: (run: () => void) => void;
}

/** The target, with the picker that fills it in when anything is running. */
interface TargetField extends Field {
  assign: HTMLElement | null;
}

function textField(name: string, caption: string, initial: string): Field {
  const field = element("label", "field");
  field.append(element("span", "field-name", caption));

  const input = document.createElement("input");
  input.className = "field-input";
  input.name = name;
  input.type = "text";
  input.value = initial;
  input.autocomplete = "off";
  field.append(input);

  return {
    field,
    value: () => input.value.trim(),
    onChange: (run) => input.addEventListener("input", run),
  };
}

function button(caption: string, variant: string): HTMLButtonElement {
  const node = document.createElement("button");
  node.type = variant === "primary" ? "submit" : "button";
  node.className = `button ${variant}`.trim();
  node.textContent = caption;
  return node;
}

function readable(gesture: string): string {
  const caption = gesture.replace(/_/g, " ");
  return caption.charAt(0).toUpperCase() + caption.slice(1);
}

/** Re-exported so the surface and inspector agree on captions. */
export { captionFor };
