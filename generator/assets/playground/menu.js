// The document menu in the editor's bar: the examples (the ones sharing a
// `menu` grouped in a submenu of that name, opened on hover, on a click
// for touch screens, or with the right arrow key), then below a line the
// visitor's sketches (on the server when signed in, and those kept in this
// browser), and "New sketch…".

import { el } from "./util.js?v=__VERSION__";

export class DocMenu {
  /**
   * `parts` holds the picker's elements (`picker`, `button`, `menu`);
   * `open(target)` is called with the chosen document, as `{kind, id}`
   * (`kind` is "example", "local", "sketch", or "new").
   */
  constructor({ picker, button, menu }, open) {
    this.picker = picker;
    this.button = button;
    this.menu = menu;
    this.open = open;
    this.current = null;
    this.bind();
  }

  /**
   * Fills the menu: `examples` (as the generator lists them), `remote` (the
   * signed-in visitor's server sketches as `{id, title}`, or null when
   * signed out), `local` (this browser's sketches), and `profileUrl` (where
   * all of the visitor's sketches are).
   */
  fill({ examples, remote, local, profileUrl }) {
    const items = [];
    const submenus = new Map();
    for (const example of examples) {
      const item = this.item(example.title, { kind: "example", id: example.id });
      if (!example.menu) {
        items.push(item);
        continue;
      }
      let submenu = submenus.get(example.menu);
      if (!submenu) {
        const parent = el("li", { role: "none", className: "has-submenu" },
          el("button", { role: "menuitem", attrs: { "aria-haspopup": "menu" } }, example.menu));
        submenu = el("ul", { className: "menu submenu", role: "menu", ariaLabel: example.menu });
        parent.append(submenu);
        items.push(parent);
        submenus.set(example.menu, submenu);
      }
      submenu.append(item);
    }
    items.push(separator());
    if (remote) {
      items.push(heading("Your sketches"));
      if (remote.length === 0) items.push(note("Nothing saved yet"));
      for (const sketch of remote.slice(0, 12)) {
        items.push(this.item(sketch.title, { kind: "sketch", id: sketch.id }));
      }
      if (remote.length > 12 && profileUrl) {
        items.push(el("li", { role: "none" },
          el("a", { role: "menuitem", href: profileUrl, className: "menu-link" }, "All your sketches →")));
      }
    }
    if (local.length > 0) {
      items.push(heading(remote ? "In this browser" : "Your sketches (this browser)"));
      for (const sketch of local) {
        items.push(this.item(sketch.title, { kind: "local", id: sketch.id }));
      }
    }
    items.push(this.item("New sketch…", { kind: "new" }, "new-sketch"));
    this.menu.replaceChildren(...items);
    this.markCurrent();
  }

  item(label, target, className = "") {
    return el("li", { role: "none", className },
      el("button", {
        role: "menuitem",
        dataset: { kind: target.kind, id: target.id ?? "" },
      }, label));
  }

  /** Shows `label` on the button and highlights the document `{kind, id}`. */
  setCurrent(label, target) {
    this.button.textContent = label;
    this.button.title = label;
    this.current = target;
    this.markCurrent();
  }

  markCurrent() {
    const { kind, id } = this.current ?? {};
    for (const item of this.menu.querySelectorAll("[data-kind]")) {
      item.classList.toggle(
        "current",
        item.dataset.kind === kind && item.dataset.id === (id ?? ""),
      );
    }
    for (const parent of this.menu.querySelectorAll(".has-submenu")) {
      parent.firstChild.classList.toggle(
        "current",
        parent.querySelector("[data-kind].current") !== null,
      );
    }
  }

  openMenu() {
    this.menu.hidden = false;
    this.button.ariaExpanded = "true";
    const current = this.menu.querySelector("[data-kind].current");
    current?.closest(".has-submenu")?.classList.add("open");
    (current ?? this.menu.querySelector("[role=menuitem]"))?.focus();
  }

  closeMenu() {
    this.menu.hidden = true;
    this.button.ariaExpanded = "false";
    this.closeSubmenus();
  }

  closeSubmenus(except) {
    for (const parent of this.menu.querySelectorAll(".has-submenu.open")) {
      if (parent !== except) parent.classList.remove("open");
    }
  }

  bind() {
    const { button, menu } = this;
    button.addEventListener("click", () => (menu.hidden ? this.openMenu() : this.closeMenu()));
    button.addEventListener("keydown", (event) => {
      if (event.key === "ArrowDown" && menu.hidden) {
        event.preventDefault();
        this.openMenu();
      }
    });
    menu.addEventListener("click", (event) => {
      const choice = event.target.closest("button[role=menuitem]");
      if (!choice) return;
      const parent = choice.parentElement;
      if (parent.classList.contains("has-submenu")) {
        this.closeSubmenus(parent);
        parent.classList.toggle("open");
        return;
      }
      this.closeMenu();
      button.focus();
      const { kind, id } = choice.dataset;
      if (kind && !(kind === this.current?.kind && id === (this.current?.id ?? ""))) {
        this.open({ kind, id: id || null });
      }
    });
    // Hovering one top-level item closes a submenu another was holding open.
    menu.addEventListener("pointerover", (event) => {
      const item = event.target.closest(`#${menu.id} > li`);
      if (item) this.closeSubmenus(item);
    });
    menu.addEventListener("keydown", (event) => this.navigate(event));
    document.addEventListener("pointerdown", (event) => {
      if (!menu.hidden && !this.picker.contains(event.target)) this.closeMenu();
    });
  }

  navigate(event) {
    const item = event.target.closest("[role=menuitem]");
    if (!item) return;
    const parent = item.parentElement;
    const items = [...item.closest("[role=menu]").children]
      .map((child) => child.firstChild)
      .filter((child) => child?.role === "menuitem");
    const index = items.indexOf(item);
    const inSubmenu = parent.parentElement !== this.menu;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      const step = event.key === "ArrowDown" ? 1 : -1;
      items[(index + step + items.length) % items.length].focus();
    } else if (event.key === "ArrowRight" && parent.classList.contains("has-submenu")) {
      parent.classList.add("open");
      parent.querySelector(".submenu [role=menuitem]").focus();
    } else if (event.key === "ArrowLeft" && inSubmenu) {
      const opener = parent.closest(".has-submenu");
      opener.classList.remove("open");
      opener.firstChild.focus();
    } else if (event.key === "Escape") {
      this.closeMenu();
      this.button.focus();
    } else if (event.key === "Tab") {
      this.closeMenu();
    } else {
      return;
    }
    event.preventDefault();
  }
}

const separator = () => el("li", { role: "separator", className: "menu-separator" });
const heading = (text) => el("li", { role: "presentation", className: "menu-heading" }, text);
const note = (text) => el("li", { role: "presentation", className: "menu-note" }, text);
