// PushOS Studio.
//
// Studio configures a running PushOS and nothing more. It holds no
// configuration of its own, so what is on screen is what PushOS actually has,
// and closing Studio changes nothing about the surface.

import {
  client,
  describeError,
  describeSurface,
  type BindingAddress,
  type BindingSpec,
  type EditReport,
  type StatusReport,
  type Vocabulary,
} from "./api";
import { usePreview } from "./bridge";
import { renderInspector } from "./ui/inspector";
import { element, renderSurface } from "./ui/surface";

/** Everything on screen. */
interface State {
  status: StatusReport | null;
  vocabulary: Vocabulary | null;
  bindings: BindingSpec[];
  /** The page being edited, or null for bindings that apply everywhere. */
  page: string | null;
  selected: string | null;
  notice: Notice | null;
}

interface Notice {
  text: string;
  detail: string[];
  tone: "good" | "bad";
}

const state: State = {
  status: null,
  vocabulary: null,
  bindings: [],
  page: null,
  selected: null,
  notice: null,
};

const root = document.querySelector<HTMLElement>("#studio");

/** Re-reads everything from PushOS and redraws. */
async function refresh(): Promise<void> {
  try {
    const api = await client();
    const [status, vocabulary, bindings] = await Promise.all([
      api.status(),
      api.describe(),
      api.bindings(),
    ]);

    state.status = status;
    state.vocabulary = vocabulary;
    state.bindings = bindings.bindings;

    // Start on the page PushOS is showing, so Studio opens where the operator
    // already is rather than somewhere arbitrary.
    if (state.page === null && status.page !== null) {
      const known = vocabulary.pages.some((page) => page.id === status.page);
      if (known) state.page = status.page;
    }
  } catch (error) {
    const failure = describeError(error);
    state.status = null;
    state.notice = {
      text: failure.message,
      detail: failure.problems,
      tone: "bad",
    };
  }

  draw();
}

/** Runs an edit, reports what happened, and re-reads. */
async function edit(
  describe: (report: EditReport) => string,
  run: () => Promise<EditReport>,
): Promise<void> {
  try {
    const report = await run();
    state.notice = { text: describe(report), detail: [], tone: "good" };
  } catch (error) {
    const failure = describeError(error);
    state.notice = { text: failure.message, detail: failure.problems, tone: "bad" };
  }
  await refresh();
}

function save(spec: BindingSpec): void {
  void edit(
    (report) =>
      `${report.replaced ? "Replaced" : "Added"} ${spec.control} ${spec.gesture} — ${
        report.binding_count
      } bindings in force`,
    async () => (await client()).bind(spec),
  );
}

function remove(address: BindingAddress): void {
  void edit(
    (report) =>
      `Removed ${address.control} ${address.gesture} — ${report.binding_count} bindings in force`,
    async () => (await client()).unbind(address),
  );
}

function test(address: BindingAddress): void {
  void (async () => {
    try {
      const report = await (await client()).test(address);
      const detail = report.message === undefined ? "" : `: ${report.message}`;
      state.notice = {
        text: `${report.action} ${report.status}${detail}`,
        detail: [],
        tone: report.status === "failed" ? "bad" : "good",
      };
    } catch (error) {
      const failure = describeError(error);
      state.notice = { text: failure.message, detail: failure.problems, tone: "bad" };
    }
    draw();
  })();
}

function draw(): void {
  if (root === null) return;
  // Clears the static starting message along with the previous frame.
  root.replaceChildren();
  root.append(header());

  if (state.status === null || state.vocabulary === null) {
    root.append(disconnected());
    return;
  }

  const body = element("div", "body");
  body.append(sidebar(state.vocabulary));

  const main = element("div", "main");
  main.append(
    renderSurface(
      {
        controls: state.vocabulary.controls,
        bindings: state.bindings,
        page: state.page,
        selected: state.selected,
      },
      (control) => {
        state.selected = control;
        draw();
      },
    ),
  );
  main.append(
    renderInspector(
      {
        control: state.selected,
        page: state.page,
        vocabulary: state.vocabulary,
        bindings: state.bindings,
      },
      { save, remove, test },
    ),
  );

  body.append(main);
  root.append(body);
}

function header(): HTMLElement {
  const bar = element("header", "bar");
  bar.append(element("h1", "product", "PushOS Studio"));

  const status = element("div", "status");
  if (usePreview()) {
    // Said plainly and first: sample data that looks real is worse than none.
    status.append(pill("Sample data — not a running PushOS", "bad"));
  }
  if (state.status === null) {
    status.append(pill("PushOS not running", "bad"));
  } else {
    status.append(pill(`PushOS ${state.status.version}`, "good"));

    const surface = describeSurface(state.status.surface);
    status.append(pill(surface.text, surface.tone));
    status.append(pill(`${state.status.binding_count} bindings`, "idle"));
  }
  bar.append(status);

  if (state.notice !== null) {
    const notice = element("div", `notice ${state.notice.tone}`);
    notice.append(element("p", "notice-text", state.notice.text));
    for (const problem of state.notice.detail) {
      notice.append(element("p", "notice-detail", problem));
    }
    bar.append(notice);
  }

  return bar;
}

function disconnected(): HTMLElement {
  const panel = element("div", "disconnected");
  panel.append(element("h2", "", "PushOS is not running"));
  panel.append(
    element(
      "p",
      "",
      "Studio configures a PushOS that is already running. Start it, then try again.",
    ),
  );
  const command = element("pre", "command", "pushos run");
  panel.append(command);

  const retry = document.createElement("button");
  retry.type = "button";
  retry.className = "button primary";
  retry.textContent = "Try again";
  retry.addEventListener("click", () => void refresh());
  panel.append(retry);

  return panel;
}

function sidebar(vocabulary: Vocabulary): HTMLElement {
  const aside = element("aside", "sidebar");
  aside.append(element("h2", "sidebar-title", "Pages"));

  const list = element("nav", "pages");
  list.append(
    pageButton("Everywhere", "Bindings that apply on every page", null),
  );
  for (const page of vocabulary.pages) {
    list.append(pageButton(page.name, page.description ?? "", page.id));
  }
  aside.append(list);

  if (state.status !== null) {
    const source = element("div", "source");
    source.append(element("h2", "sidebar-title", "Configuration"));
    const path = element("p", "source-path", state.status.config_root);
    path.title = state.status.config_root;
    source.append(path);
    source.append(
      element(
        "p",
        "source-note",
        "These files are the source of truth. Studio edits them in place and keeps your comments.",
      ),
    );
    aside.append(source);
  }

  return aside;
}

function pageButton(name: string, detail: string, id: string | null): HTMLElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "page";
  if (state.page === id) button.classList.add("current");
  button.setAttribute("aria-pressed", String(state.page === id));

  button.append(element("span", "page-name", name));
  if (detail !== "") button.append(element("span", "page-detail", detail));

  button.addEventListener("click", () => {
    state.page = id;
    draw();
  });
  return button;
}

function pill(text: string, tone: string): HTMLElement {
  return element("span", `pill ${tone}`, text);
}

/** Renders anything that escapes, so a failure is never a blank window. */
function reportFatal(detail: string): void {
  if (root === null) return;
  root.replaceChildren();

  const panel = element("div", "disconnected");
  panel.append(element("h2", "", "Studio could not start"));
  panel.append(
    element("p", "", "Something went wrong before anything could be drawn."),
  );
  panel.append(element("pre", "command", detail));
  root.append(panel);
}

window.addEventListener("error", (event) => reportFatal(String(event.message)));
window.addEventListener("unhandledrejection", (event) =>
  reportFatal(String(event.reason)),
);

refresh().catch((error: unknown) => reportFatal(describeError(error).message));
