// A sketch's files in the editor: the file explorer (a tree of the
// sketch's folders and files, beside the editor, collapsible) and the tabs
// above the editor, one per file that's open. Opening a file from the
// explorer adds its tab; closing a tab leaves the file in the sketch.

import { el, storage } from "./util.js?v=__VERSION__";

/** The folders and files under `prefix` (a folder path ending in `/`, or
 * ""), as `{folders: [name], files: [name]}`, each sorted. */
function children(paths, folders, prefix) {
  const childFolders = new Set();
  const childFiles = [];
  for (const path of [...paths, ...folders.map((folder) => `${folder}/`)]) {
    if (!path.startsWith(prefix)) continue;
    const rest = path.slice(prefix.length);
    const slash = rest.indexOf("/");
    if (slash >= 0) childFolders.add(rest.slice(0, slash));
    else if (rest) childFiles.push(rest);
  }
  const byName = (a, b) => a.localeCompare(b);
  return { folders: [...childFolders].sort(byName), files: childFiles.sort(byName) };
}

const icon = (name) => el("span", { className: `tree-icon ${name}`, attrs: { "aria-hidden": "true" } });

export class FileExplorer {
  /**
   * `host` is the explorer's element. `actions` has `open(path)`,
   * `newFile(folder)`, `newFolder(folder)`, `rename(path, isFolder)`,
   * `remove(path, isFolder)` (where `folder` is a folder path, or "" for
   * the top level), and `refresh()`, which renders the explorer again.
   */
  constructor(host, actions) {
    this.host = host;
    this.actions = actions;
    // Folders the visitor has collapsed, by path.
    this.collapsed = new Set();
    this.list = el("ul", { className: "tree", role: "tree", ariaLabel: "Files" });
    host.replaceChildren(
      el("div", { className: "explorer-head" },
        el("span", { className: "explorer-title" }, "Files"),
        el("button", { className: "icon-button", title: "New file", attrs: { "aria-label": "New file" }, onclick: () => actions.newFile("") }, icon("new-file")),
        el("button", { className: "icon-button", title: "New folder", attrs: { "aria-label": "New folder" }, onclick: () => actions.newFolder("") }, icon("new-folder"))),
      this.list,
    );
  }

  /**
   * Shows `doc`'s folders and files, with `active` (the file in the editor)
   * highlighted, the files in `withProblems` (a set of paths) marked, and
   * the files in `withEntries` (a set of paths) marked as holding entry
   * points.
   */
  render(doc, active, withProblems = new Set(), withEntries = new Set()) {
    const paths = doc.files.map((file) => file.path);
    const rows = [];
    const walk = (prefix, depth) => {
      const { folders, files } = children(paths, doc.folders, prefix);
      for (const name of folders) {
        const path = `${prefix}${name}`;
        const open = !this.collapsed.has(path);
        rows.push(this.folderRow(path, name, depth, open));
        if (open) walk(`${path}/`, depth + 1);
      }
      for (const name of files) {
        const path = `${prefix}${name}`;
        rows.push(this.fileRow(path, name, depth, {
          active: path === active,
          problems: withProblems.has(path),
          entries: withEntries.has(path),
        }));
      }
    };
    walk("", 0);
    this.list.replaceChildren(...rows);
  }

  folderRow(path, name, depth, open) {
    const toggle = () => {
      if (open) this.collapsed.add(path);
      else this.collapsed.delete(path);
      this.actions.refresh();
    };
    return el("li", {
      className: "tree-row folder",
      role: "treeitem",
      attrs: { "aria-expanded": String(open) },
      style: { "--depth": depth },
    },
      el("button", { className: "tree-name", onclick: toggle, title: path },
        el("span", { className: `tree-chevron${open ? " open" : ""}`, attrs: { "aria-hidden": "true" } }),
        icon("folder"), name),
      el("span", { className: "tree-actions" },
        el("button", { className: "icon-button", title: `New file in ${path}`, attrs: { "aria-label": `New file in ${path}` }, onclick: () => this.actions.newFile(path) }, icon("new-file")),
        el("button", { className: "icon-button", title: `Rename ${path}`, attrs: { "aria-label": `Rename ${path}` }, onclick: () => this.actions.rename(path, true) }, icon("rename")),
        el("button", { className: "icon-button", title: `Delete ${path}`, attrs: { "aria-label": `Delete ${path}` }, onclick: () => this.actions.remove(path, true) }, icon("delete"))));
  }

  fileRow(path, name, depth, { active, problems, entries }) {
    return el("li", {
      className: `tree-row file${active ? " active" : ""}${problems ? " has-problems" : ""}`,
      role: "treeitem",
      attrs: { "aria-selected": String(active) },
      style: { "--depth": depth },
    },
      el("button", { className: "tree-name", onclick: () => this.actions.open(path), title: path },
        el("span", { className: "tree-chevron none", attrs: { "aria-hidden": "true" } }),
        icon("file"), name,
        entries ? el("span", { className: "tree-entry", title: "Has @cpu entry points" }, "▶") : null),
      el("span", { className: "tree-actions" },
        el("button", { className: "icon-button", title: `Rename ${path}`, attrs: { "aria-label": `Rename ${path}` }, onclick: () => this.actions.rename(path, false) }, icon("rename")),
        el("button", { className: "icon-button", title: `Delete ${path}`, attrs: { "aria-label": `Delete ${path}` }, onclick: () => this.actions.remove(path, false) }, icon("delete"))));
  }
}

/** Whether the explorer is shown, remembered across visits. */
export const explorerOpen = {
  get: () => storage.get("explorer") !== "closed",
  set: (open) => storage.set("explorer", open ? "open" : "closed"),
};

export class FileTabs {
  /**
   * `host` is the strip's element; `actions` has `select(path)` and
   * `close(path)`.
   */
  constructor(host, actions) {
    this.host = host;
    this.actions = actions;
  }

  /** Shows a tab for each of `open` (paths), `active` selected, and marks
   * the files in `withProblems`. */
  render(open, active, withProblems = new Set()) {
    const tabs = open.map((path) => {
      const slash = path.lastIndexOf("/");
      return el(
        "div",
        {
          className: `file-tab${path === active ? " active" : ""}${withProblems.has(path) ? " has-problems" : ""}`,
          role: "tab",
          title: path,
          attrs: { "aria-selected": String(path === active) },
          onauxclick: (event) => {
            if (event.button === 1) this.actions.close(path);
          },
        },
        el("button", { className: "file-select", onclick: () => this.actions.select(path) },
          el("span", { className: "file-name" },
            slash >= 0 ? el("span", { className: "file-dir" }, path.slice(0, slash + 1)) : null,
            path.slice(slash + 1))),
        el("button", {
          className: "file-close",
          title: `Close ${path}`,
          attrs: { "aria-label": `Close ${path}` },
          onclick: (event) => {
            event.stopPropagation();
            this.actions.close(path);
          },
        }, "×"),
      );
    });
    this.host.replaceChildren(...tabs);
    this.host.querySelector(".file-tab.active")?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }
}
