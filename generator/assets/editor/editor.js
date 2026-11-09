// The editor page (server/templates/editor.html, on the editor's own host):
// a Monaco editor for a sketch's easl files beside a canvas running the
// program.
//
// - The page opens a document (see documents.js): a saved sketch
//   (/s/<id>) or a new one (/new), as the server's page data says.
// - A sketch's files (in folders, if it likes) are listed in the file
//   explorer beside the editor (files.js); the tabs above the editor are
//   the files that are open.
// - Language support comes from the analysis worker (analysis.js,
//   features.js), which checks the file being edited along with the other
//   files it imports.
// - A sketch can define several `@cpu` entry points, in any of its files.
//   The bar above the canvas (with the run, pause, and stop buttons) lists
//   them, and each one has a ▶ in the editor's gutter (and a link in its
//   `@cpu` annotation's hover) to run it; the sketch remembers the one it
//   runs.
// - Each run happens in a fresh iframe (run.js); a program only runs once
//   the check of the file holding its entry point is clean.
// - Edits are kept in this browser as drafts until they're saved to the
//   server.

import { registerEasl, currentTheme, indentAfter } from "./easl-language.js?v=__VERSION__";
import { $, IS_MAC, el, pathProblem, promptText, storage, toast } from "./util.js?v=__VERSION__";
import { api } from "./api.js?v=__VERSION__";
import { Analysis, quietly } from "./analysis.js?v=__VERSION__";
import { Checker, rangeMaker, registerFeatures } from "./features.js?v=__VERSION__";
import { forgetDraft, newDoc, sketchDoc } from "./documents.js?v=__VERSION__";
import { FileExplorer, FileTabs, explorerOpen } from "./files.js?v=__VERSION__";
import { Runner, renderEntries, supportsWebGPU } from "./run.js?v=__VERSION__";
import { Social } from "./social.js?v=__VERSION__";
import { setUpHelp, setUpSplitters } from "./layout.js?v=__VERSION__";

const page = JSON.parse($("page-data").textContent);
const user = page.user;
const MONACO = document.documentElement.dataset.monaco;

// The program a new sketch starts with.
const starter = await fetch(new URL("new-sketch.easl?v=__VERSION__", import.meta.url))
  .then((response) => (response.ok ? response.text() : ""))
  .catch(() => "");

// A program a link carries (`/new#code=<base64url>`, as the docs link to
// their programs), as `{source, encoded, id}`. Its id comes from its text,
// so reopening the same link restores its edits.
const linkedCode = (() => {
  const encoded = new URLSearchParams(location.hash.slice(1)).get("code");
  if (page.mode !== "new" || !encoded) return null;
  try {
    const base64 = encoded.replace(/-/g, "+").replace(/_/g, "/");
    const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
    const source = new TextDecoder().decode(bytes);
    return { source, encoded, id: `code-${hash(source)}` };
  } catch {
    return null;
  }
})();

/** A short hash of `text` (32-bit FNV-1a, in hex). */
function hash(text) {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(16);
}

// ---- Monaco ----

function loadMonaco() {
  return new Promise((resolve, reject) => {
    if (typeof require === "undefined") {
      reject(new Error("the editor couldn't be loaded"));
      return;
    }
    // Monaco's own workers come from the CDN too, which a page can only
    // start through a same-origin script.
    window.MonacoEnvironment = {
      getWorkerUrl: () =>
        "data:text/javascript;charset=utf-8," +
        encodeURIComponent(
          `self.MonacoEnvironment = { baseUrl: "${MONACO}/" };` +
            `importScripts("${MONACO}/vs/base/worker/workerMain.js");`,
        ),
    };
    require.config({ paths: { vs: `${MONACO}/vs` } });
    require(["vs/editor/editor.main"], () => resolve(window.monaco), reject);
  });
}

const analysis = new Analysis();
let monaco;
try {
  monaco = await loadMonaco();
} catch (error) {
  $("editor").textContent =
    `The editor couldn't be loaded (${error.message}). It comes from ` +
    `cdn.jsdelivr.net; check that the page can reach it.`;
  throw error;
}
registerEasl(monaco);
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () =>
  monaco.editor.setTheme(currentTheme()),
);
const toRange = rangeMaker(monaco);

// ---- the open document ----

let doc = null;
// The file shown in the editor.
let activePath = null;
// Changes whenever any of the document's files does (a check describes
// one revision).
let revision = 0;
// Each model, by document key and file path.
const models = new Map();
// The `@cpu` entry points in each file of the open document, by path, as
// `[{name, range}]`.
let entriesByPath = new Map();
// The paths of the files whose last check found problems.
const filesWithProblems = new Set();

function modelFor(target, path) {
  let byPath = models.get(target.key);
  if (!byPath) models.set(target.key, (byPath = new Map()));
  let model = byPath.get(path);
  const file = target.file(path);
  if (!file) return null;
  if (!model) {
    model = monaco.editor.createModel(
      file.text,
      "easl",
      monaco.Uri.from({ scheme: "easl", path: `/${encodeURIComponent(target.key)}/${path}` }),
    );
    model.path = path;
    model.docKey = target.key;
    model.onDidChangeContent(() => fileEdited(model));
    byPath.set(path, model);
  } else if (model.getValue() !== file.text) {
    model.setValue(file.text);
  }
  return model;
}

/** Drops `target`'s model of `path` (after the file is deleted or renamed). */
function dropModel(target, path) {
  const model = models.get(target.key)?.get(path);
  if (!model) return;
  models.get(target.key).delete(path);
  model.dispose();
}

const activeModel = () => (doc && activePath ? modelFor(doc, activePath) : null);

let persistTimer = null;

function fileEdited(model) {
  if (!doc || model.docKey !== doc.key) return;
  doc.setText(model.path, model.getValue());
  documentChanged([model.path]);
}

/** Everything that follows a change to the open document's files (the
 * paths whose text changed, or all of them). */
function documentChanged(paths = doc.files.map((file) => file.path)) {
  revision++;
  clearTimeout(persistTimer);
  persistTimer = setTimeout(() => doc.persist(), 400);
  updateBar();
  setStatus("analyzing", "Checking…");
  checker.schedule(activeModel());
  updateEntries(paths);
}

function flushPersist() {
  if (persistTimer !== null) {
    clearTimeout(persistTimer);
    persistTimer = null;
    doc?.persist();
  }
}
addEventListener("pagehide", flushPersist);

// ---- checking ----

const checker = new Checker({
  monaco,
  analysis,
  doc: () => doc,
  revision: () => revision,
  modelFor,
  onChecked: (result) => {
    if (result.doc !== doc) return;
    filesWithProblems.delete(result.model.path);
    if (result.failure || result.diagnostics.length > 0) filesWithProblems.add(result.model.path);
    renderFiles();
    if (result.model !== activeModel()) return;
    if (result.failure) {
      setStatus("failure", "Couldn't check");
    } else if (result.diagnostics.length === 0) {
      setStatus("ok", "No problems");
    } else {
      const count = result.diagnostics.length;
      setStatus("errors", `${count} problem${count === 1 ? "" : "s"}`);
    }
  },
});

function setStatus(kind, text) {
  const status = $("status");
  status.dataset.kind = kind;
  status.textContent = text;
}

$("status").addEventListener("click", () => {
  if (checker.checked.diagnostics.length > 0) {
    editor.focus();
    editor.trigger("status", "editor.action.marker.next", null);
  }
});

// ---- entry points ----

const RUN_ENTRY_COMMAND = "easl.runEntry";

/** Every `@cpu` entry point of the document, in file order, as
 * `{file, name, range}`. */
function allEntries() {
  return doc.files.flatMap((file) =>
    (entriesByPath.get(file.path) ?? []).map((entry) => ({ file: file.path, ...entry })),
  );
}

/** The entry point a run starts: the document's chosen one while it's
 * still there, else its first. */
function effectiveEntry() {
  const entries = allEntries();
  return (
    entries.find((e) => e.file === doc.entry?.file && e.name === doc.entry?.name) ??
    entries[0] ??
    null
  );
}

let entriesRequest = 0;

/** Finds the entry points of the files at `paths` again. */
async function updateEntries(paths) {
  const request = ++entriesRequest;
  const target = doc;
  const found = await Promise.all(
    paths.map(async (path) => [
      path,
      (await quietly(analysis.request("cpuEntries", [target.file(path)?.text ?? ""]))) ?? [],
    ]),
  );
  if (request !== entriesRequest || target !== doc) return;
  for (const [path, entries] of found) entriesByPath.set(path, entries);
  showEntries();
}

const entryDecorations = { collection: null };

function showEntries() {
  const current = effectiveEntry();
  renderEntries($("entries"), {
    entries: allEntries(),
    current,
    multipleFiles: doc.files.length > 1,
    choose: (entry) => chooseEntry(entry),
  });
  const decorations = (entriesByPath.get(activePath) ?? []).map((entry) => {
    const isCurrent = current?.file === activePath && current.name === entry.name;
    return {
      range: new monaco.Range(entry.range[0] + 1, 1, entry.range[0] + 1, 1),
      options: {
        glyphMarginClassName: `entry-glyph${isCurrent ? " current" : ""}`,
        glyphMarginHoverMessage: { value: `Run \`${entry.name}\`` },
      },
    };
  });
  entryDecorations.collection ??= editor.createDecorationsCollection();
  entryDecorations.collection.set(decorations);
  renderFiles();
}

/** The entry point whose `@cpu` annotation is at `position` in `model`. */
function entryAt(model, position) {
  if (model !== activeModel()) return null;
  const entry = (entriesByPath.get(model.path) ?? []).find((e) => {
    const range = toRange(e.range);
    return range.containsPosition(position);
  });
  return entry ? { file: model.path, ...entry } : null;
}

/** Makes `entry` the one the document runs, and runs it. */
function chooseEntry(entry) {
  doc.entry = { file: entry.file, name: entry.name };
  doc.persist();
  updateBar();
  showEntries();
  run();
}

// ---- the editor ----

const editor = monaco.editor.create($("editor"), {
  theme: currentTheme(),
  automaticLayout: true,
  fontFamily: 'ui-monospace, "SF Mono", SFMono-Regular, Menlo, Consolas, monospace',
  fontSize: 13.5,
  lineHeight: 20,
  tabSize: 2,
  insertSpaces: true,
  detectIndentation: false,
  minimap: { enabled: false },
  scrollBeyondLastLine: false,
  padding: { top: 12, bottom: 12 },
  stickyScroll: { enabled: false },
  fixedOverflowWidgets: true,
  glyphMargin: true,
  bracketPairColorization: { enabled: true },
  guides: { bracketPairs: "active", indentation: false },
  matchBrackets: "always",
  autoClosingBrackets: "always",
  autoClosingQuotes: "always",
  autoClosingOvertype: "always",
  autoClosingDelete: "always",
  autoSurround: "languageDefined",
  wordBasedSuggestions: "off",
  "semanticHighlighting.enabled": true,
});

editor.onMouseDown((event) => {
  if (event.target.type !== monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN) return;
  const line = event.target.position.lineNumber;
  const entry = (entriesByPath.get(activePath) ?? []).find((e) => e.range[0] + 1 === line);
  if (entry) chooseEntry({ file: activePath, ...entry });
});

if (typeof monaco.editor.registerCommand === "function") {
  monaco.editor.registerCommand(RUN_ENTRY_COMMAND, (_accessor, file, name) =>
    chooseEntry({ file, name }),
  );
}

// Go-to-definition into another of the document's files opens that file
// (once the navigation that asked has finished, which switching models
// under it would cancel).
monaco.editor.registerEditorOpener({
  openCodeEditor(_source, resource, selectionOrPosition) {
    const model = monaco.editor.getModel(resource);
    if (!model || model.docKey !== doc?.key) return false;
    setTimeout(() => {
      showFile(model.path);
      if (!selectionOrPosition) return;
      if ("startLineNumber" in selectionOrPosition) {
        editor.setSelection(selectionOrPosition);
        editor.revealRangeInCenter(selectionOrPosition);
      } else {
        editor.setPosition(selectionOrPosition);
        editor.revealPositionInCenter(selectionOrPosition);
      }
      editor.focus();
    });
    return true;
  },
});

await registerFeatures({
  monaco,
  analysis,
  checker,
  modelFor,
  doc: () => doc,
  revision: () => revision,
  entryAt,
  runEntryCommand: RUN_ENTRY_COMMAND,
});

// ---- editing commands ----

const fromSelection = (selection) => [
  selection.startLineNumber - 1,
  selection.startColumn - 1,
  selection.endLineNumber - 1,
  selection.endColumn - 1,
];

/** Runs one of easl_lsp's structural commands on the selection. */
async function structural(command) {
  const model = editor.getModel();
  const selection = editor.getSelection();
  const version = model.getVersionId();
  const result = await quietly(
    analysis.request("structural", [command, model.getValue(), ...fromSelection(selection)]),
  );
  if (!result || model.getVersionId() !== version) return;
  if (command === "moveFormForward" || command === "moveFormBackward") {
    editor.pushUndoStop();
    editor.executeEdits("easl", [{ range: toRange(result.range), text: result.newText }]);
    editor.pushUndoStop();
    editor.setSelection(toRange(result.selection));
  } else {
    const [line, character] = result;
    editor.setPosition({ lineNumber: line + 1, column: character + 1 });
  }
  editor.revealPosition(editor.getPosition());
}

const { KeyMod, KeyCode } = monaco;
// Control on every platform (on a Mac, Cmd belongs to the browser).
const CTRL = IS_MAC ? KeyMod.WinCtrl : KeyMod.CtrlCmd;
// On a Mac, the browser's shortcuts use ⌘, so the Control bindings
// easl_lsp's VS Code extension uses are free too (Windows and Linux
// browsers reserve Ctrl+W, and Ctrl+A and Ctrl+D mean select-all and
// bookmark).
const macOnly = (...keybindings) => (IS_MAC ? keybindings : []);

for (const action of [
  {
    id: "easl.expandSelection",
    label: "Easl: Expand Selection to Enclosing Form",
    keybindings: macOnly(KeyMod.WinCtrl | KeyCode.KeyW),
    run: () => editor.trigger("easl", "editor.action.smartSelect.expand", null),
  },
  {
    id: "easl.run",
    label: "Run the program",
    keybindings: [KeyMod.CtrlCmd | KeyCode.Enter],
    run: () => run(),
  },
  {
    id: "easl.save",
    label: "Save: format (if Format on save is on), save, and run",
    keybindings: [KeyMod.CtrlCmd | KeyCode.KeyS],
    run: () => saveShortcut(),
  },
  {
    id: "easl.newFile",
    label: "Easl: Add a File to the Sketch",
    run: () => addFile(),
  },
  {
    id: "easl.moveFormForward",
    label: "Easl: Move Form Forward",
    keybindings: [CTRL | KeyMod.Alt | KeyCode.RightArrow, ...macOnly(KeyMod.WinCtrl | KeyMod.Shift | KeyCode.KeyD)],
    run: () => structural("moveFormForward"),
  },
  {
    id: "easl.moveFormBackward",
    label: "Easl: Move Form Backward",
    keybindings: [CTRL | KeyMod.Alt | KeyCode.LeftArrow, ...macOnly(KeyMod.WinCtrl | KeyMod.Shift | KeyCode.KeyA)],
    run: () => structural("moveFormBackward"),
  },
  {
    id: "easl.moveCursorToStart",
    label: "Easl: Move Cursor to Start of Current Form",
    keybindings: [CTRL | KeyMod.Alt | KeyCode.UpArrow, ...macOnly(KeyMod.WinCtrl | KeyCode.KeyA)],
    run: () => structural("moveCursorToStart"),
  },
  {
    id: "easl.moveCursorToEnd",
    label: "Easl: Move Cursor to End of Current Form",
    keybindings: [CTRL | KeyMod.Alt | KeyCode.DownArrow, ...macOnly(KeyMod.WinCtrl | KeyCode.KeyD)],
    run: () => structural("moveCursorToEnd"),
  },
]) {
  editor.addAction(action);
}

// Enter indents the new line by its position in the forms around it: two
// past an unclosed `(`, one past an unclosed `[` or `{`.
editor.addCommand(
  KeyCode.Enter,
  () => {
    const model = editor.getModel();
    const edits = editor.getSelections().map((selection) => {
      const before = model.getValueInRange(
        new monaco.Range(1, 1, selection.startLineNumber, selection.startColumn),
      );
      return { range: selection, text: "\n" + " ".repeat(indentAfter(before)), forceMoveMarkers: true };
    });
    editor.pushUndoStop();
    editor.executeEdits("easl", edits, (inverse) =>
      inverse.map(
        (edit) =>
          new monaco.Selection(
            edit.range.endLineNumber,
            edit.range.endColumn,
            edit.range.endLineNumber,
            edit.range.endColumn,
          ),
      ),
    );
    editor.pushUndoStop();
  },
  "editorTextFocus && !suggestWidgetVisible && !inSnippetMode && !editorReadonly",
);

// ---- files ----

// The files open in tabs, in order.
let openFiles = [];

const explorer = new FileExplorer($("explorer"), {
  open: (path) => showFile(path),
  newFile: (folder) => addFile(folder),
  newFolder: (folder) => addFolder(folder),
  rename: (path, isFolder) => (isFolder ? renameFolder(path) : renameFile(path)),
  remove: (path, isFolder) => (isFolder ? removeFolder(path) : removeFile(path)),
  move: (path, isFolder, target) => moveInTree(path, isFolder, target),
  refresh: () => renderFiles(),
});

const fileTabs = new FileTabs($("file-tabs"), {
  select: (path) => showFile(path),
  close: (path) => closeFile(path),
});

/** Shows the explorer and the tabs as the document is now. */
function renderFiles() {
  if (!doc) return;
  const withEntries = new Set(
    doc.files.map((file) => file.path).filter((path) => (entriesByPath.get(path) ?? []).length > 0),
  );
  explorer.render(doc, activePath, filesWithProblems, withEntries);
  fileTabs.render(openFiles, activePath, filesWithProblems);
}

/** Shows or hides the explorer, remembering the choice when it's the
 * visitor's (`remember`). */
function setExplorerOpen(open, remember = true) {
  if (remember) explorerOpen.set(open);
  document.body.classList.toggle("explorer-closed", !open);
  document.body.classList.remove("explorer-peeking");
  $("explorer-toggle").setAttribute("aria-expanded", String(open));
  $("explorer-toggle").title = open ? "Hide the files" : "Show the files";
}
$("explorer-toggle").addEventListener("click", () =>
  setExplorerOpen(document.body.classList.contains("explorer-closed")),
);
// While it's collapsed, the explorer slides out when the pointer reaches the
// window's left edge, and goes away when the pointer leaves it (after a
// moment, so a pointer that slips off briefly doesn't close it).
let peekTimer = null;
const peek = (peeking) => {
  clearTimeout(peekTimer);
  if (peeking) document.body.classList.add("explorer-peeking");
  else peekTimer = setTimeout(() => document.body.classList.remove("explorer-peeking"), 250);
};
$("explorer-peek-zone").addEventListener("pointerenter", () => peek(true));
$("explorer").addEventListener("pointerenter", () => {
  if (document.body.classList.contains("explorer-peeking")) peek(true);
});
$("explorer").addEventListener("pointerleave", () => peek(false));
$("explorer-peek-zone").addEventListener("pointerleave", (event) => {
  if (!$("explorer").contains(event.relatedTarget)) peek(false);
});

// On a phone, the editor needs the whole width, so the explorer starts
// closed there.
setExplorerOpen(explorerOpen.get() && !matchMedia("(max-width: 40rem)").matches, false);

/** Shows the document's file at `path` in the editor, opening its tab. */
function showFile(path) {
  if (!doc.file(path)) return;
  activePath = path;
  if (!openFiles.includes(path)) openFiles.push(path);
  editor.setModel(modelFor(doc, path));
  editor.updateOptions({ readOnly: false });
  $("editor-empty").hidden = true;
  showEntries();
  setStatus("analyzing", "Checking…");
  checker.schedule(activeModel(), 0);
}

/** Closes `path`'s tab (the file stays in the sketch). */
function closeFile(path) {
  const index = openFiles.indexOf(path);
  if (index < 0) return;
  openFiles.splice(index, 1);
  if (activePath !== path) {
    renderFiles();
    return;
  }
  const next = openFiles[Math.min(index, openFiles.length - 1)];
  if (next) {
    showFile(next);
  } else {
    activePath = null;
    editor.setModel(null);
    $("editor-empty").hidden = false;
    setStatus("analyzing", "");
    showEntries();
  }
}

/** Every folder of the document: the empty ones it keeps, and every
 * folder holding a file. */
function allFolders() {
  const folders = new Set(doc.folders);
  for (const file of doc.files) {
    const parts = file.path.split("/");
    for (let i = 1; i < parts.length; i++) folders.add(parts.slice(0, i).join("/"));
  }
  return folders;
}

const takenProblem = (path, except = null) =>
  path !== except && (doc.file(path) || allFolders().has(path))
    ? "There's already a file or folder with that name."
    : null;

const filePathProblem = (path, except = null) => pathProblem(path) ?? takenProblem(path, except);

function folderPathProblem(path, except = null) {
  if (!path) return "A folder needs a name.";
  if (!path.split("/").every((segment) => /^[A-Za-z0-9._-]+$/.test(segment) && segment !== "." && segment !== "..")) {
    return "Use only letters, digits, and . _ - in folder names, separated by single slashes.";
  }
  return takenProblem(path, except);
}

/** `folder` (a folder path or "") as a prefix for paths inside it. */
const inside = (folder) => (folder ? `${folder}/` : "");

/** Adds a file, asking for its name: in `folder` (a folder path, or "" for
 * the top level), where the name can also include folders of its own. */
async function addFile(folder = "") {
  const name = await promptText({
    title: folder ? `New file in ${folder}/` : "New file",
    label: "File name (it can include folders, like lib/noise.easl)",
    value: "",
    confirm: "Add file",
    validate: (name) => filePathProblem(`${inside(folder)}${name}`),
  });
  if (!name) return;
  const path = `${inside(folder)}${name}`;
  doc.files.push({ path, text: "" });
  doc.folders = doc.folders.filter((f) => !path.startsWith(`${f}/`));
  documentChanged([path]);
  showFile(path);
  editor.focus();
  toast(`Import it from another file with (import "${path}")`);
}

async function addFolder(parent = "") {
  const name = await promptText({
    title: parent ? `New folder in ${parent}/` : "New folder",
    label: "Folder name",
    value: "",
    confirm: "Add folder",
    validate: (name) => folderPathProblem(`${inside(parent)}${name.replace(/\/+$/, "")}`),
  });
  if (!name) return;
  doc.folders.push(`${inside(parent)}${name.replace(/\/+$/, "")}`);
  doc.persist();
  renderFiles();
}

/** Moves the file at `from` to `to`, keeping its model's place in the
 * tabs and the entry point that names it. */
function moveFile(from, to) {
  const file = doc.file(from);
  dropModel(doc, from);
  file.path = to;
  if (doc.entry?.file === from) doc.entry = { ...doc.entry, file: to };
  entriesByPath.set(to, entriesByPath.get(from) ?? []);
  entriesByPath.delete(from);
  filesWithProblems.delete(from);
  openFiles = openFiles.map((path) => (path === from ? to : path));
  if (activePath === from) activePath = to;
}

async function renameFile(path) {
  const renamed = await promptText({
    title: "Rename file",
    label: "File name (change its folders to move it)",
    value: path,
    confirm: "Rename",
    validate: (name) => filePathProblem(name, path),
  });
  if (!renamed || renamed === path) return;
  moveFile(path, renamed);
  documentChanged();
  if (activePath) showFile(activePath);
}

async function renameFolder(folder) {
  const renamed = await promptText({
    title: "Rename folder",
    label: "Folder name (change its parent folders to move it)",
    value: folder,
    confirm: "Rename",
    validate: (name) => folderPathProblem(name.replace(/\/+$/, ""), folder),
  });
  const target = renamed?.replace(/\/+$/, "");
  if (!target || target === folder) return;
  const prefix = `${folder}/`;
  for (const file of [...doc.files]) {
    if (file.path.startsWith(prefix)) moveFile(file.path, `${target}/${file.path.slice(prefix.length)}`);
  }
  doc.folders = doc.folders.map((f) =>
    f === folder ? target : f.startsWith(prefix) ? `${target}/${f.slice(prefix.length)}` : f,
  );
  documentChanged();
  if (activePath) showFile(activePath);
  else renderFiles();
}

const parentOf = (path) => path.slice(0, Math.max(0, path.lastIndexOf("/")));
const baseName = (path) => path.slice(path.lastIndexOf("/") + 1);

/**
 * Moves a file or folder dragged in the explorer to `target`: before or
 * after a file or folder (into the folder holding it), into a folder (at
 * its end), or, when `target` is null, to the end of the top level. The
 * sketch's file order follows, so it's saved too.
 */
function moveInTree(path, isFolder, target) {
  const folder =
    target === null ? "" : target.where === "into" ? target.path : parentOf(target.path);
  const moved = `${inside(folder)}${baseName(path)}`;
  if (isFolder && (folder === path || folder.startsWith(`${path}/`))) return;
  if (moved !== path && (doc.file(moved) || allFolders().has(moved))) {
    toast(`There's already a file or folder named ${moved}.`, "error");
    return;
  }
  // The files moving (a folder's, in their order), renamed.
  const prefix = `${path}/`;
  const movedFiles = doc.files.filter((file) =>
    isFolder ? file.path.startsWith(prefix) : file.path === path,
  );
  const wasEmptyFolder = isFolder && movedFiles.length === 0;
  const rename = (from) => (isFolder ? `${moved}/${from.slice(prefix.length)}` : moved);
  for (const file of movedFiles) {
    if (file.path !== rename(file.path)) moveFile(file.path, rename(file.path));
  }
  if (isFolder) {
    doc.folders = doc.folders.map((f) =>
      f === path ? moved : f.startsWith(prefix) ? `${moved}/${f.slice(prefix.length)}` : f,
    );
  }
  // The folder they left stays, even if it's empty now.
  const left = parentOf(path);
  if (left && left !== folder && !allFolders().has(left)) doc.folders.push(left);
  // Their place in the order.
  const rest = doc.files.filter((file) => !movedFiles.includes(file));
  let index = rest.length;
  if (target !== null && !wasEmptyFolder) {
    const within = (file) =>
      target.folder ? file.path.startsWith(`${target.path}/`) : file.path === target.path;
    const first = rest.findIndex(within);
    const last = rest.findLastIndex(within);
    if (target.where === "before" && first >= 0) index = first;
    else if (first >= 0) index = last + 1;
  }
  rest.splice(index, 0, ...movedFiles);
  doc.files = rest;
  documentChanged();
  if (activePath) showFile(activePath);
  else renderFiles();
}

function removeFile(path) {
  if (doc.files.length === 1) {
    toast("A sketch needs at least one file.", "error");
    return;
  }
  if (doc.file(path).text.trim() && !confirm(`Delete ${path}?`)) return;
  doc.files = doc.files.filter((file) => file.path !== path);
  // Its folder stays, even if this was the last file in it.
  const slash = path.lastIndexOf("/");
  if (slash > 0 && !allFolders().has(path.slice(0, slash))) doc.folders.push(path.slice(0, slash));
  dropModel(doc, path);
  entriesByPath.delete(path);
  filesWithProblems.delete(path);
  if (doc.entry?.file === path) doc.entry = null;
  closeFile(path);
  documentChanged();
}

function removeFolder(folder) {
  const prefix = `${folder}/`;
  const inFolder = doc.files.filter((file) => file.path.startsWith(prefix));
  if (inFolder.length === doc.files.length) {
    toast("A sketch needs at least one file outside the folders it deletes.", "error");
    return;
  }
  if (inFolder.length > 0 &&
      !confirm(`Delete ${folder} and the ${inFolder.length} file${inFolder.length === 1 ? "" : "s"} in it?`)) {
    return;
  }
  for (const file of inFolder) {
    dropModel(doc, file.path);
    entriesByPath.delete(file.path);
    filesWithProblems.delete(file.path);
    if (doc.entry?.file === file.path) doc.entry = null;
    closeFile(file.path);
  }
  doc.files = doc.files.filter((file) => !file.path.startsWith(prefix));
  doc.folders = doc.folders.filter((f) => f !== folder && !f.startsWith(prefix));
  documentChanged();
  renderFiles();
}

// ---- the bar ----

/** The bar's buttons for the open document. */
function updateBar() {
  const owned = doc.canSave;
  const dirty = doc.isDirty();
  const save = $("save");
  save.hidden = !(owned || doc.kind === "new");
  save.disabled = owned && !dirty;
  save.textContent = owned && !dirty ? "Saved" : "Save";
  $("fork").hidden = !(doc.kind === "sketch" && !owned);
  $("reset").hidden = !(doc.kind === "sketch" && dirty);
  $("share").hidden = doc.kind !== "sketch";
  document.body.dataset.dirty = String(dirty);
}

// ---- opening documents ----

function openDoc(next, { run: shouldRun = false } = {}) {
  runner.stop();
  doc = next;
  revision++;
  entriesByPath = new Map();
  filesWithProblems.clear();
  hideNotice();
  activePath = doc.entry?.file && doc.file(doc.entry.file) ? doc.entry.file : doc.files[0].path;
  openFiles = [activePath];
  // Models of this document from earlier take its files as they are now.
  for (const file of doc.files) modelFor(doc, file.path);
  editor.setModel(modelFor(doc, activePath));
  $("editor-empty").hidden = true;
  renderFiles();
  document.title = `${doc.title} — easl`;
  const url =
    doc.kind === "sketch" && page.playlist && page.sketch?.id === doc.id
      ? `${doc.url}?playlist=${encodeURIComponent(page.playlist.id)}`
      : doc.url;
  history.replaceState(null, "", url);
  renderPlaylistNav();
  social.show(doc);
  updateBar();
  setStatus("analyzing", "Checking…");
  checker.schedule(activeModel(), 0);
  updateEntries(doc.files.map((file) => file.path)).then(() => {
    if (shouldRun && supportsWebGPU && next === doc) run();
  });
}

/** Opens `doc` again with its files replaced by `files` (after they
 * change other than by editing, like a revert). */
function reloadFiles() {
  for (const file of doc.files) {
    const model = models.get(doc.key)?.get(file.path);
    if (model && model.getValue() !== file.text) model.setValue(file.text);
  }
  for (const [path] of models.get(doc.key) ?? []) {
    if (!doc.file(path)) dropModel(doc, path);
  }
  openFiles = openFiles.filter((path) => doc.file(path));
  if (!doc.file(activePath)) activePath = doc.files[0].path;
  documentChanged();
  showFile(activePath);
}

function offerStaleDraft(draft) {
  showNotice("You have unsaved edits to an older version of this sketch.", [
    ["Restore them", () => {
      doc.files = draft.files.map((file) => ({ ...file }));
      doc.entry = draft.entry;
      doc.folders = [...draft.folders];
      reloadFiles();
      doc.persist();
      hideNotice();
    }],
    ["Discard", () => {
      storage.remove(`draft:${doc.key}`);
      hideNotice();
    }],
  ]);
}

function showNotice(text, actions) {
  $("notice").replaceChildren(
    el("span", {}, text),
    ...actions.map(([label, action]) => el("button", { className: "small", onclick: action }, label)),
  );
  $("notice").hidden = false;
}

function hideNotice() {
  $("notice").hidden = true;
}

function renderPlaylistNav() {
  const nav = $("playlist-nav");
  const playlist = page.playlist;
  if (!playlist || doc.kind !== "sketch" || page.sketch?.id !== doc.id) {
    nav.hidden = true;
    return;
  }
  const link = (id, label, className) =>
    id
      ? el("a", { href: `/s/${encodeURIComponent(id)}?playlist=${encodeURIComponent(playlist.id)}`, className }, label)
      : el("span", { className: `${className} disabled` }, label);
  nav.replaceChildren(
    el("a", { href: `/p/${encodeURIComponent(playlist.id)}`, className: "playlist-name" }, playlist.title),
    el("span", { className: "playlist-position" }, `${playlist.index + 1} of ${playlist.count}`),
    link(playlist.prev, "‹ Previous", "playlist-prev"),
    link(playlist.next, "Next ›", "playlist-next"),
  );
  nav.hidden = false;
}

// ---- running ----

const runner = new Runner({
  reveal: (path, line) => {
    showFile(path);
    editor.revealLineInCenter(line);
    editor.setPosition({ lineNumber: line, column: 1 });
    editor.focus();
  },
});

async function run() {
  if (!supportsWebGPU || !doc) return;
  const entry = effectiveEntry();
  if (!entry) {
    runner.stop();
    runner.clearConsole();
    runner.log("error", "This sketch has no @cpu entry point to run. Mark a function that takes no arguments with @cpu.");
    return;
  }
  const target = doc;
  await runner.run({
    files: doc.files,
    entry,
    check: async () => {
      const result = await checker.check(modelFor(target, entry.file));
      // Hover and go-to-definition answer from the last check, so the file
      // in the editor is checked again when the entry is elsewhere.
      if (entry.file !== activePath) checker.schedule(activeModel(), 0);
      return result;
    },
  });
}

$("run").addEventListener("click", () => run());
$("stop").addEventListener("click", () => runner.stop());
$("pause").addEventListener("click", () => runner.togglePause());
$("clear").addEventListener("click", () => runner.clearConsole());
$("format").addEventListener("click", () => {
  editor.focus();
  editor.getAction("editor.action.formatDocument").run();
});

// ---- saving, forking, and the rest of a document's life ----

const formatOnSave = $("format-on-save");
formatOnSave.checked = storage.get("format-on-save") !== "false";
formatOnSave.addEventListener("change", () =>
  storage.set("format-on-save", String(formatOnSave.checked)),
);

/**
 * Ctrl/Cmd+S: formats the file when Format on save is on (the format
 * command applies the smallest edits it can, so the cursor stays put, and
 * one undo reverts it; a file that doesn't parse is left as it is), saves
 * a sketch its owner is editing (and a new one), and runs the program.
 */
async function saveShortcut() {
  if (formatOnSave.checked) await editor.getAction("editor.action.formatDocument").run();
  if (doc.canSave || doc.kind === "new") await save({ quiet: true });
  run();
}

function signIn() {
  flushPersist();
  location.href = `/login?next=${encodeURIComponent(location.pathname + location.search)}`;
}

/** Saves the open document: an owned sketch's changes go to the server,
 * and a new sketch is saved to the visitor's account (signing in first). */
async function save({ quiet = false } = {}) {
  if (doc.kind === "new") return saveNew();
  if (!doc.canSave || !doc.isDirty()) return;
  const saving = doc;
  $("save").disabled = true;
  $("save").textContent = "Saving…";
  try {
    const sketch = await api.updateSketch(saving.id, saving.forServer());
    saving.markSaved(sketch);
    if (saving === doc) {
      social.show(doc);
      updateBar();
    }
    if (!quiet) toast("Saved");
    refreshThumbnail(sketch.id);
  } catch (error) {
    updateBar();
    toast(error.message, "error");
  }
}

async function saveNew() {
  if (!user) return signIn();
  let title = doc.title;
  if (title === "Untitled sketch") {
    title = await promptText({
      title: "Save sketch",
      label: "Name",
      value: title,
      confirm: "Save",
      validate: (value) => (value ? null : "A sketch needs a name."),
    });
    if (!title) return;
  }
  try {
    const saving = doc;
    const sketch = await api.createSketch({ title, ...saving.forServer() });
    forgetDraft(saving);
    page.sketch = sketch;
    openDoc(sketchDoc(sketch).doc, { run: true });
    toast("Saved. It's private until you change its visibility in About.");
    refreshThumbnail(sketch.id);
  } catch (error) {
    toast(error.message, "error");
  }
}

/** Saves a copy of the open sketch as a new sketch of the visitor's. */
async function fork() {
  if (!user) return signIn();
  if (doc.kind !== "sketch") return;
  try {
    const forked = doc;
    const sketch = await api.forkSketch(forked.id, { title: `Fork of ${forked.title}`, ...forked.forServer() });
    forked.revert();
    page.sketch = sketch;
    openDoc(sketchDoc(sketch).doc, { run: true });
    toast("Saved your copy. It's private until you change its visibility in About.");
    refreshThumbnail(sketch.id);
  } catch (error) {
    toast(error.message, "error");
  }
}

async function renameDoc() {
  if (!(doc.kind === "new" || doc.canSave)) return;
  const title = await promptText({
    title: "Rename sketch",
    label: "Name",
    value: doc.title,
    confirm: "Rename",
    validate: (value) => (value ? null : "A sketch needs a name."),
  });
  if (!title || title === doc.title) return;
  if (doc.kind === "sketch") {
    try {
      const sketch = await api.updateSketch(doc.id, { title });
      doc.sketch = { ...doc.sketch, title: sketch.title, updated_at: sketch.updated_at };
      // The draft (if any) was made from the version before the rename.
      doc.persist();
    } catch (error) {
      toast(error.message, "error");
      return;
    }
  }
  doc.title = title;
  document.title = `${doc.title} — easl`;
  social.show(doc);
}

async function deleteDoc() {
  if (!doc.canSave) return;
  if (!confirm(`Delete “${doc.title}”? Its comments and likes go too. This can't be undone.`)) return;
  try {
    await api.deleteSketch(doc.id);
    doc.revert();
    location.href = `/u/${encodeURIComponent(user.username)}`;
  } catch (error) {
    toast(error.message, "error");
  }
}

function revert() {
  if (!confirm("Discard the changes you haven't saved?")) return;
  doc.revert();
  reloadFiles();
}

/** Takes a picture of the running program for the sketch's thumbnail,
 * once it has run a moment. */
async function refreshThumbnail(sketchId, wait = 1500) {
  const deadline = Date.now() + 8000;
  while (runner.state !== "running" && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  await new Promise((resolve) => setTimeout(resolve, wait));
  if (doc?.id !== sketchId || runner.state !== "running") return false;
  const picture = await runner.capture();
  if (!picture) return false;
  try {
    await api.uploadThumbnail(sketchId, picture);
    return true;
  } catch {
    return false;
  }
}

async function shareLink() {
  const sketch = doc.sketch;
  if (sketch.visibility === "private") {
    toast(
      sketch.can_edit
        ? "This sketch is private: make it unlisted or public (in About) to share it."
        : "This sketch is private.",
      "error",
    );
    social.selectTab("about");
    return;
  }
  const url = new URL(`/s/${sketch.id}`, location.origin).href;
  try {
    await navigator.clipboard.writeText(url);
    toast("Link copied");
  } catch {
    promptText({ title: "Share", label: "Link to this sketch", value: url, confirm: "Done" });
  }
}

$("save").addEventListener("click", () => save());
$("fork").addEventListener("click", () => fork());
$("reset").addEventListener("click", () => revert());
$("share").addEventListener("click", () => shareLink());

// Ctrl/Cmd+Enter runs and Ctrl/Cmd+S saves from anywhere on the page, not
// just the editor.
document.addEventListener("keydown", (event) => {
  if (event.defaultPrevented || !(event.ctrlKey || event.metaKey)) return;
  if (event.key === "Enter") {
    event.preventDefault();
    run();
  } else if (event.key === "s") {
    event.preventDefault();
    saveShortcut();
  }
});

const social = new Social(user, {
  save: () => save(),
  fork: () => fork(),
  renameDoc: () => renameDoc(),
  deleteDoc: () => deleteDoc(),
  signIn,
  updateThumbnail: async () => {
    const ok = await refreshThumbnail(doc.id, 0);
    toast(ok ? "Thumbnail updated" : "Run the sketch first, then try again.", ok ? "info" : "error");
  },
});

// ---- the visitor ----

function renderAccount() {
  const host = $("account");
  if (!user) {
    host.replaceChildren(
      el("a", { href: `/login?next=${encodeURIComponent(location.pathname + location.search)}`, className: "account-link" }, "Sign in"),
    );
    return;
  }
  const menuList = el("div", { className: "account-menu", hidden: true },
    el("a", { href: `/u/${encodeURIComponent(user.username)}` }, "Your profile"),
    el("a", { href: "/settings" }, "Settings"),
    user.is_admin ? el("a", { href: "/admin" }, "Moderation") : null,
    el("form", { method: "post", action: "/logout" }, el("button", { type: "submit" }, "Sign out")));
  const button = el("button", {
    className: "account-button",
    title: user.username,
    attrs: { "aria-haspopup": "menu", "aria-label": `Account: ${user.username}` },
    onclick: () => (menuList.hidden = !menuList.hidden),
  }, el("img", { src: user.avatar_url, alt: "", className: "avatar" }));
  document.addEventListener("pointerdown", (event) => {
    if (!host.contains(event.target)) menuList.hidden = true;
  });
  host.replaceChildren(button, menuList);
}

// ---- start ----

setUpSplitters();
setUpHelp();
renderAccount();
if (!supportsWebGPU) {
  document.body.classList.add("no-webgpu");
  $("run").disabled = true;
}
runner.setState("stopped");

if (page.mode === "sketch" && page.sketch) {
  const { doc: opened, staleDraft } = sketchDoc(page.sketch);
  openDoc(opened, { run: true });
  if (staleDraft) offerStaleDraft(staleDraft);
} else {
  openDoc(newDoc(starter, linkedCode), { run: true });
}
editor.focus();
