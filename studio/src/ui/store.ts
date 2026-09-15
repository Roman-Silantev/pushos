// The store: the packs PushOS can install, and what each would do.
//
// Installing is shown before it is offered. Every pack is reviewed first, and
// the review says what it adds, what it asks to be allowed, and why it would
// not work here if it would not. The install button sends back exactly the
// permissions the review showed, so nothing is agreed to that was not read.

import type { PackEntry, PackReview } from "../api";
import { element } from "./surface";

/** What the store shows. */
export interface StoreModel {
  packs: PackEntry[];
  /** The pack being looked at closely, once its review has arrived. */
  review: PackReview | null;
  /** The pack whose review or install is under way, so it is not asked twice. */
  busy: string | null;
}

/** What the store can ask for. */
export interface StoreActions {
  review: (pack: PackEntry) => void;
  install: (review: PackReview) => void;
  close: () => void;
}

export function renderStore(model: StoreModel, actions: StoreActions): HTMLElement {
  const store = element("section", "store");

  const heading = element("div", "store-heading");
  heading.append(element("h2", "store-title", "Store"));
  heading.append(
    element(
      "p",
      "store-note",
      "Packs of agents, pages and bindings. Nothing changes until you have read what one adds and installed it.",
    ),
  );
  store.append(heading);

  if (model.packs.length === 0) {
    store.append(element("p", "store-note", "No packs are on offer."));
    return store;
  }

  const list = element("ul", "pack-list");
  for (const pack of model.packs) {
    list.append(card(pack, model, actions));
  }
  store.append(list);

  if (model.review !== null) store.append(reviewPanel(model.review, model.busy, actions));
  return store;
}

function card(pack: PackEntry, model: StoreModel, actions: StoreActions): HTMLElement {
  const looking = model.review?.pack.id === pack.id;
  const item = element("li", `pack${looking ? " current" : ""}`);

  const heading = element("div", "pack-heading");
  heading.append(element("span", "pack-name", pack.name));
  heading.append(element("span", `pack-state ${pack.state}`, stateName(pack)));
  item.append(heading);

  item.append(element("p", "pack-summary", pack.summary));
  const byline = pack.author === undefined ? `v${pack.version}` : `v${pack.version} · ${pack.author}`;
  item.append(element("p", "pack-byline", byline));

  const review = document.createElement("button");
  review.type = "button";
  review.className = "button";
  review.disabled = model.busy === pack.id;
  review.textContent = model.busy === pack.id ? "Reading…" : looking ? "Reviewing" : "Review";
  review.addEventListener("click", () => actions.review(pack));
  item.append(review);

  return item;
}

function reviewPanel(
  review: PackReview,
  busy: string | null,
  actions: StoreActions,
): HTMLElement {
  const panel = element("div", "pack-review");

  const heading = element("div", "pack-heading");
  heading.append(element("h3", "pack-name", review.pack.name));
  const close = document.createElement("button");
  close.type = "button";
  close.className = "button";
  close.textContent = "Close";
  close.addEventListener("click", actions.close);
  heading.append(close);
  panel.append(heading);

  panel.append(section("Adds", [review.adds]));

  panel.append(
    section(
      "Asks to be allowed",
      review.granting.length === 0 ? ["Nothing new."] : review.granting,
    ),
  );
  if (review.already.length > 0) {
    panel.append(section("Already allowed", review.already));
  }
  if (review.missing_providers.length > 0) {
    panel.append(
      section(
        "Expects agents that are not set up",
        review.missing_providers.map((provider) => `${provider}: its roles cannot start until it is`),
      ),
    );
  }
  if (review.problems.length > 0) {
    panel.append(section("Would not install here", review.problems, "bad"));
  }

  const footer = element("div", "pack-actions");
  if (review.pack.state === "available") {
    const install = document.createElement("button");
    install.type = "button";
    install.className = "button primary";
    const blocked = review.problems.length > 0 || busy === review.pack.id;
    install.disabled = blocked;
    install.textContent =
      busy === review.pack.id
        ? "Installing…"
        : review.granting.length === 0
          ? "Install"
          : `Allow ${review.granting.length} and install`;
    install.addEventListener("click", () => actions.install(review));
    footer.append(install);
  } else {
    footer.append(element("p", "store-note", `${stateName(review.pack)} here.`));
  }
  panel.append(footer);

  return panel;
}

function section(title: string, lines: string[], tone = ""): HTMLElement {
  const block = element("div", `pack-section ${tone}`.trim());
  block.append(element("h4", "pack-section-title", title));
  const list = element("ul", "pack-lines");
  for (const line of lines) list.append(element("li", "", line));
  block.append(list);
  return block;
}

function stateName(pack: PackEntry): string {
  switch (pack.state) {
    case "installed":
      return "Installed";
    case "disabled":
      return "Installed, switched off";
    case "available":
      return "Available";
  }
}
