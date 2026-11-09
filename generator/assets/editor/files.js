// A sketch's files in the editor: the file explorer (a tree of the
// sketch's folders and files, beside the editor, collapsible) and the tabs
// above the editor, one per file that's open. Opening a file from the
// explorer adds its tab; closing a tab leaves the file in the sketch.
//
// The tree shows the sketch's files in their order (a folder where its
// first file is), and rows can be dragged: onto the top or bottom half of a
// file to go before or after it (in its folder), onto the middle of a
// folder to go in it, and onto the space below the rows to go to the top
// level, at the end.

import { el, storage } from "./util.js?v=__VERSION__";

/** The folders and files directly under `prefix` (a folder path ending in
 * `/`, or ""), in the order of `paths` (a folder where its first file is;
 * folders holding no file, `folders`, after the files), as
 * `[{name, folder}]`. */
function children(paths, folders, prefix) {
  const entries = [];
  const seenFolders = new Set();
  for (const path of [...paths, ...folders.map((folder) => `${folder}/`)]) {
    if (!path.startsWith(prefix)) continue;
    const rest = path.slice(prefix.length);
    const slash = rest.indexOf("/");
    if (slash >= 0) {
      const name = rest.slice(0, slash);
      if (!seenFolders.has(name)) {
        seenFolders.add(name);
        entries.push({ name, folder: true });
      }
    } else if (rest) {
      entries.push({ name: rest, folder: false });
    }
  }
  return entries;
}

/** Every folder holding a file in `paths`, plus `folders`. */
function allFolders(paths, folders) {
  const all = new Set(folders);
  for (const path of paths) {
    const parts = path.split("/");
    for (let i = 1; i < parts.length; i++) all.add(parts.slice(0, i).join("/"));
  }
  return [...all];
}

const icon = (name) => el("span", { className: `tree-icon ${name}`, attrs: { "aria-hidden": "true" } });

export class FileExplorer {
  /**
   * `host` is the explorer's element. `actions` has `open(path)`,
   * `newFile(folder)`, `newFolder(folder)`, `rename(path, isFolder)`,
   * `remove(path, isFolder)` (where `folder` is a folder path, or "" for
   * the top level), `move(path, isFolder, target)` (a dragged row dropped
   * on `target`, `{path, folder, where}` with `where` "before", "after",
   * or "into", or null for the end of the top level), and `refresh()`,
   * which renders the explorer again.
   */
  constructor(host, actions) {
    this.host = host;
    this.actions = actions;
    // Folders the visitor has collapsed, by path.
    this.collapsed = new Set();
    this.list = el("ul", { className: "tree", role: "tree", ariaLabel: "Files" });
    // The row being dragged, as `{path, folder}`.
    this.dragging = null;
    // Dropping below the rows moves to the top level.
    host.addEventListener("dragover", (event) => {
      if (!this.dragging || event.target.closest(".tree-row")) return;
      event.preventDefault();
      this.clearDropMarks();
      this.list.classList.add("drop-end");
    });
    host.addEventListener("dragleave", (event) => {
      if (!host.contains(event.relatedTarget)) this.clearDropMarks();
    });
    host.addEventListener("drop", (event) => {
      if (!this.dragging || event.target.closest(".tree-row")) return;
      event.preventDefault();
      const dragged = this.dragging;
      this.endDrag();
      this.actions.move(dragged.path, dragged.folder, null);
    });
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
      for (const { name, folder } of children(paths, doc.folders, prefix)) {
        const path = `${prefix}${name}`;
        if (folder) {
          const open = !this.collapsed.has(path);
          rows.push(this.draggable(this.folderRow(path, name, depth, open), path, true));
          if (open) walk(`${path}/`, depth + 1);
        } else {
          rows.push(this.draggable(this.fileRow(path, name, depth, {
            active: path === active,
            problems: withProblems.has(path),
            entries: withEntries.has(path),
          }), path, false));
        }
      }
    };
    walk("", 0);
    // Without folders, files need no room for a folder's chevron.
    this.list.classList.toggle("flat", allFolders(paths, doc.folders).length === 0);
    this.list.replaceChildren(...rows);
  }

  /** Makes `row` (the row of `path`) draggable, and a place to drop
   * others. */
  draggable(row, path, folder) {
    row.draggable = true;
    row.addEventListener("dragstart", (event) => {
      this.dragging = { path, folder };
      event.dataTransfer.effectAllowed = "move";
      event.dataTransfer.setData("text/plain", path);
      row.classList.add("dragging");
    });
    row.addEventListener("dragend", () => this.endDrag());
    row.addEventListener("dragover", (event) => {
      const where = this.dropPlace(event, row, path, folder);
      if (!where) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = "move";
      this.clearDropMarks();
      row.classList.add(`drop-${where}`);
    });
    row.addEventListener("drop", (event) => {
      const where = this.dropPlace(event, row, path, folder);
      if (!where) return;
      event.preventDefault();
      const dragged = this.dragging;
      this.endDrag();
      this.actions.move(dragged.path, dragged.folder, { path, folder, where });
    });
    return row;
  }

  /** Where a row being dragged over `row` would go: "before", "after", or
   * (on a folder's middle) "into" it, by the pointer's height on the row;
   * or null where it can't go (onto itself, or into its own folder). */
  dropPlace(event, row, path, folder) {
    const dragged = this.dragging;
    if (!dragged || dragged.path === path) return null;
    if (dragged.folder && path.startsWith(`${dragged.path}/`)) return null;
    const box = row.getBoundingClientRect();
    const height = (event.clientY - box.top) / box.height;
    if (folder) return height < 0.25 ? "before" : height > 0.75 ? "after" : "into";
    return height < 0.5 ? "before" : "after";
  }

  clearDropMarks() {
    this.list.classList.remove("drop-end");
    for (const marked of this.list.querySelectorAll(".drop-before, .drop-after, .drop-into")) {
      marked.classList.remove("drop-before", "drop-after", "drop-into");
    }
  }

  endDrag() {
    this.dragging = null;
    this.clearDropMarks();
    this.list.querySelector(".dragging")?.classList.remove("dragging");
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
        el("span", { className: "tree-label" }, name)),
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
        el("span", { className: "tree-label" }, name),
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
