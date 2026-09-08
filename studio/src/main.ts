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
  type SessionInfo,
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
  /** What is running now. Refreshed on its own, because it changes while the
   *  rest of the screen is being edited. */
  sessions: SessionInfo[];
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
  sessions: [],
};

const root = document.querySelector<HTMLElement>("#studio");

/** How often the running sessions are re-read, in milliseconds. */
const SESSION_POLL_MS = 3000;

/** Re-reads everything from PushOS and redraws. */
async function refresh(): Promise<void> {
  try {
    const api = await client();
    const [status, vocabulary, bindings, sessions] = await Promise.all([
      api.status(),
      api.describe(),
      api.bindings(),
      api.sessions(),
    ]);

    state.status = status;
    state.vocabulary = vocabulary;
    state.bindings = bindings.bindings;
    state.sessions = sessions.sessions;

    // Start on the page PushOS is showing, so Studio opens where the operator
    // already is rather than somewhere arbitrary.
    if (state.page === null && status.page !== null) {
      const known = vocabulary.pages.some((page) => page.id === status.page);
      if (known) state.page = status.page;
    }
  } catch (error) {
    const failure = describeError(error);
    state.status = null;
    state.sessions = [];
    state.notice = {
      text: failure.message,
      detail: failure.problems,
      tone: "bad",
    };
  }

  draw();
}

/**
 * Re-reads only what is running, and redraws only that panel.
 *
 * A full redraw would discard whatever the operator has half-typed into the
 * inspector, so this touches one node and leaves the forms alone.
 */
async function refreshSessions(): Promise<void> {
  if (state.status === null) return;

  try {
    const { sessions } = await (await client()).sessions();
    state.sessions = sessions;
  } catch {
    // A session list that could not be read is not worth a banner: the next
    // edit will report the failure properly, and the panel says it is empty.
    state.sessions = [];
  }

  const panel = document.querySelector<HTMLElement>(".running");
  panel?.replaceWith(runningPanel());
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
        sessions: state.sessions,
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

  aside.append(runningPanel());
  return aside;
}

/**
 * What is running now.
 *
 * Here rather than in the inspector because it answers a different question:
 * not "what does this pad do" but "what is there to point a pad at".
 */
function runningPanel(): HTMLElement {
  const panel = element("div", "running");
  panel.append(element("h2", "sidebar-title", "Running now"));

  if (state.sessions.length === 0) {
    panel.append(
      element(
        "p",
        "source-note",
        "No agents or terminals are running. Start one, and it can be bound to any control.",
      ),
    );
    return panel;
  }

  const list = element("ul", "session-list");
  for (const session of state.sessions) {
    const item = element("li", `session ${session.live ? "live" : "ended"}`);
    if (session.selected) item.classList.add("chosen");

    const heading = element("div", "session-heading");
    heading.append(element("span", "session-name", session.name));
    heading.append(element("span", "session-kind", session.kind));
    item.append(heading);

    item.append(element("p", "session-status", session.status));
    if (session.detail !== undefined) {
      const detail = element("p", "session-detail", session.detail);
      detail.title = session.detail;
      item.append(detail);
    }

    const target = session.standing_target ?? session.target;
    const bind = element("p", "session-target", target);
    bind.title = `Choose this under Target to bind a control to it: ${target}`;
    item.append(bind);

    list.append(item);
  }
  panel.append(list);
  return panel;
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

// What is running changes on its own, so the panel keeps up without the
// operator having to ask. Only that panel is redrawn, so an edit in progress is
// never thrown away.
setInterval(() => void refreshSessions(), SESSION_POLL_MS);
