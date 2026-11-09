// The strip of file tabs above the editor: one per file of the open
// document, in order, with a button to add a file. Double-clicking a tab
// renames its file, and its × deletes it (a document keeps at least one).

import { el } from "./util.js?v=__VERSION__";

export class FileTabs {
  /**
   * `host` is the strip's element; `actions` has `select(path)`, `add()`,
   * `rename(path)` and `remove(path)`.
   */
  constructor(host, actions) {
    this.host = host;
    this.actions = actions;
  }

  /** Shows `doc`'s files, `active` selected, and marks the files in
   * `withProblems` (a set of paths). */
  render(doc, active, withProblems = new Set()) {
    const single = doc.files.length === 1;
    const tabs = doc.files.map(({ path }) => {
      const slash = path.lastIndexOf("/");
      const label = el(
        "span",
        { className: "file-name" },
        slash >= 0 ? el("span", { className: "file-dir" }, path.slice(0, slash + 1)) : null,
        path.slice(slash + 1),
      );
      const tab = el(
        "div",
        {
          className: `file-tab${path === active ? " active" : ""}${withProblems.has(path) ? " has-problems" : ""}`,
          role: "tab",
          title: `${path} (double-click to rename)`,
          attrs: { "aria-selected": String(path === active) },
        },
        el(
          "button",
          {
            className: "file-select",
            onclick: () => this.actions.select(path),
            ondblclick: () => this.actions.rename(path),
          },
          label,
        ),
        single
          ? null
          : el(
              "button",
              {
                className: "file-close",
                title: `Delete ${path}`,
                attrs: { "aria-label": `Delete ${path}` },
                onclick: (event) => {
                  event.stopPropagation();
                  this.actions.remove(path);
                },
              },
              "×",
            ),
      );
      return tab;
    });
    const add = el(
      "button",
      {
        className: "file-add",
        title: "Add a file",
        attrs: { "aria-label": "Add a file" },
        onclick: () => this.actions.add(),
      },
      "+",
    );
    this.host.replaceChildren(...tabs, add);
    this.host.querySelector(".file-tab.active")?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }
}
