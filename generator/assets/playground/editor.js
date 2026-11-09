// The editor page (server/templates/editor.html): a Monaco editor for a
// sketch's easl files beside a canvas running the program.
//
// - The page opens a document (see documents.js): one of the docs'
//   examples (/playground), a new sketch (/new), or a saved sketch
//   (/s/<id>), as the server's page data says, and can switch between
//   documents in place from the menu in its bar.
// - Language support comes from the analysis worker (analysis.js,
//   features.js), which checks the file being edited along with the other
//   files it imports.
// - A document can define several `@cpu` entry points, in any of its
//   files. The bar above the canvas lists them, and each one has a ▶ in
//   the editor's gutter (and a link in its `@cpu` annotation's hover) to
//   run it; the document remembers the one it runs.
// - Each run happens in a fresh iframe (run.js); a program only runs once
//   the check of the file holding its entry point is clean.
// - Edits are kept in this browser (as drafts, or as the sketches kept
//   only here) until they're saved to the server.

import { registerEasl, currentTheme, indentAfter } from "./easl-language.js?v=__VERSION__";
import { $, IS_MAC, el, pathProblem, promptText, storage, toast } from "./util.js?v=__VERSION__";
import { api } from "./api.js?v=__VERSION__";
import { Analysis, quietly } from "./analysis.js?v=__VERSION__";
import { Checker, rangeMaker, registerFeatures } from "./features.js?v=__VERSION__";
import {
  createLocalSketch,
  deleteLocalSketch,
  exampleDoc,
  forgetNewDraft,
  localDoc,
  localSketches,
  newDoc,
  renameLocalSketch,
  sketchDoc,
} from "./documents.js?v=__VERSION__";
import { FileTabs } from "./files.js?v=__VERSION__";
import { DocMenu } from "./menu.js?v=__VERSION__";
import { Runner, renderEntries, supportsWebGPU } from "./run.js?v=__VERSION__";
import { Social } from "./social.js?v=__VERSION__";
import { setUpHelp, setUpSplitters } from "./layout.js?v=__VERSION__";

const page = JSON.parse($("page-data").textContent);
const user = page.user;
const MONACO = document.documentElement.dataset.monaco;

const catalog = await fetch(new URL("examples.json?v=__VERSION__", import.meta.url))
  .then((response) => response.json())
  .catch(() => ({ default: null, newSketch: "", examples: [] }));
const examples = [...catalog.examples];

// A program linked from the docs (`#code=<base64url>`) is one more example.
// Its id comes from its text, so reopening the same link restores its
// edits.
const linkedExample = (() => {
  const encoded = new URLSearchParams(location.hash.slice(1)).get("code");
  if (page.mode !== "playground" || !encoded) return null;
  try {
    const base64 = encoded.replace(/-/g, "+").replace(/_/g, "/");
    const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
    const source = new TextDecoder().decode(bytes);
    return { id: `linked-${hash(source)}`, title: "The program you opened", source, menu: null, linked: encoded };
  } catch {
    return null;
  }
})();
if (linkedExample) examples.unshift(linkedExample);
const examplesById = new Map(examples.map((example) => [example.id, example]));

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
    fileTabs.render(doc, activePath, filesWithProblems);
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

const fileTabs = new FileTabs($("file-tabs"), {
  select: (path) => showFile(path),
  add: () => addFile(),
  rename: (path) => renameFile(path),
  remove: (path) => removeFile(path),
});

/** Shows the document's file at `path` in the editor. */
function showFile(path) {
  if (!doc.file(path)) return;
  activePath = path;
  editor.setModel(modelFor(doc, path));
  fileTabs.render(doc, activePath, filesWithProblems);
  showEntries();
  setStatus("analyzing", "Checking…");
  checker.schedule(activeModel(), 0);
}

const otherPathProblem = (path, except = null) =>
  pathProblem(path) ??
  (path !== except && doc.file(path) ? "There's already a file with that name." : null);

async function addFile() {
  const path = await promptText({
    title: "New file",
    label: "File name (it can include folders, like lib/noise.easl)",
    value: "",
    confirm: "Add file",
    validate: (path) => otherPathProblem(path),
  });
  if (!path) return;
  doc.files.push({ path, text: "" });
  documentChanged([path]);
  showFile(path);
  editor.focus();
  toast(`Import it from another file with (import "${path}")`);
}

async function renameFile(path) {
  const renamed = await promptText({
    title: "Rename file",
    label: "File name",
    value: path,
    confirm: "Rename",
    validate: (name) => otherPathProblem(name, path),
  });
  if (!renamed || renamed === path) return;
  const file = doc.file(path);
  dropModel(doc, path);
  file.path = renamed;
  if (doc.entry?.file === path) doc.entry = { ...doc.entry, file: renamed };
  entriesByPath.set(renamed, entriesByPath.get(path) ?? []);
  entriesByPath.delete(path);
  filesWithProblems.delete(path);
  if (activePath === path) activePath = renamed;
  documentChanged();
  showFile(activePath);
}

function removeFile(path) {
  if (doc.files.length === 1) return;
  if (doc.file(path).text.trim() && !confirm(`Delete ${path}?`)) return;
  doc.files = doc.files.filter((file) => file.path !== path);
  dropModel(doc, path);
  entriesByPath.delete(path);
  filesWithProblems.delete(path);
  if (doc.entry?.file === path) doc.entry = null;
  if (activePath === path) activePath = doc.files[0].path;
  documentChanged();
  showFile(activePath);
}

// ---- the bar ----

const menu = new DocMenu(
  { picker: $("doc-picker"), button: $("doc-button"), menu: $("doc-menu") },
  (target) => openTarget(target),
);

let remoteSketches = null;

async function fillMenu() {
  menu.fill({
    examples,
    remote: user ? remoteSketches ?? [] : null,
    local: localSketches(),
    profileUrl: user ? `/u/${encodeURIComponent(user.username)}` : null,
  });
}

async function refreshRemoteSketches() {
  if (!user) return;
  const result = await quietly(api.mySketches());
  if (result) {
    remoteSketches = result.sketches;
    fillMenu();
  }
}

/** The bar's buttons for the open document. */
function updateBar() {
  const kind = doc.kind;
  const owned = doc.canSave;
  const dirty = doc.isDirty();
  const save = $("save");
  save.hidden = !(owned || kind === "new");
  save.disabled = owned && !dirty;
  save.textContent = owned && !dirty ? "Saved" : "Save";
  const fork = $("fork");
  fork.hidden = !(kind === "example" || (kind === "sketch" && !owned));
  fork.textContent = kind === "example" ? "Save a copy" : "Fork";
  $("rename").hidden = !(kind === "local" || owned);
  const reset = $("reset");
  reset.hidden = !((kind === "example" || kind === "sketch") && dirty);
  reset.textContent = kind === "example" ? "Reset" : "Revert";
  reset.title =
    kind === "example"
      ? "Discard your changes to this example"
      : "Discard the changes you haven't saved";
  $("share").hidden = kind !== "sketch";
  document.body.dataset.dirty = String(dirty);
}

// ---- opening documents ----

/** Opens the document `{kind, id}` chosen from the menu. */
async function openTarget({ kind, id }) {
  flushPersist();
  switch (kind) {
    case "example":
      openDoc(exampleDoc(examplesById.get(id)), { run: true });
      break;
    case "local":
      openDoc(localDoc(id, catalog.newSketch), { run: true });
      break;
    case "new":
      openDoc(newDoc(catalog.newSketch), { run: true });
      break;
    case "sketch":
      try {
        const sketch = await api.getSketch(id);
        const { doc: opened, staleDraft } = sketchDoc(sketch);
        openDoc(opened, { run: true });
        if (staleDraft) offerStaleDraft(staleDraft);
      } catch (error) {
        toast(error.message, "error");
      }
      break;
  }
}

function openDoc(next, { run: shouldRun = false } = {}) {
  runner.stop();
  doc = next;
  revision++;
  entriesByPath = new Map();
  filesWithProblems.clear();
  hideNotice();
  activePath = doc.entry?.file && doc.file(doc.entry.file) ? doc.entry.file : doc.files[0].path;
  // Models of this document from earlier take its files as they are now.
  for (const file of doc.files) modelFor(doc, file.path);
  editor.setModel(modelFor(doc, activePath));
  fileTabs.render(doc, activePath, filesWithProblems);
  menu.setCurrent(doc.title, { kind: doc.kind, id: doc.id });
  document.title = `${doc.title} — easl`;
  const url = doc.kind === "example" && doc.example.linked
    ? `/playground#code=${doc.example.linked}`
    : doc.kind === "sketch" && page.playlist && page.sketch?.id === doc.id
      ? `${doc.url}?playlist=${encodeURIComponent(page.playlist.id)}`
      : doc.url;
  history.replaceState(null, "", url);
  if (doc.kind === "example" || doc.kind === "local") storage.set("last", doc.kind === "example" ? doc.id : `local:${doc.id}`);
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
  if (!doc.file(activePath)) activePath = doc.files[0].path;
  documentChanged();
  showFile(activePath);
}

function offerStaleDraft(draft) {
  showNotice("You have unsaved edits to an older version of this sketch.", [
    ["Restore them", () => {
      doc.files = draft.files.map((file) => ({ ...file }));
      doc.entry = draft.entry;
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
 * and a new sketch is saved to the visitor's account (or, signed out, in
 * this browser if they'd like). */
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
    refreshRemoteSketches();
    refreshThumbnail(sketch.id);
  } catch (error) {
    updateBar();
    toast(error.message, "error");
  }
}

async function saveNew() {
  if (!user) {
    const choice = await chooseSignedOutSave();
    if (choice === "sign-in") signIn();
    if (choice === "local") {
      const local = createLocalSketch(doc.files, doc.entry);
      forgetNewDraft();
      fillMenu();
      openDoc(localDoc(local.id, catalog.newSketch));
      toast(`Saved as “${local.title}” in this browser`);
    }
    return;
  }
  const title = await promptText({
    title: "Save sketch",
    label: "Name",
    value: "Untitled sketch",
    confirm: "Save",
    validate: (value) => (value ? null : "A sketch needs a name."),
  });
  if (!title) return;
  try {
    const sketch = await api.createSketch({ title, ...doc.forServer() });
    forgetNewDraft();
    page.sketch = sketch;
    openDoc(sketchDoc(sketch).doc, { run: true });
    toast("Saved. It's private until you change its visibility in About.");
    refreshRemoteSketches();
    refreshThumbnail(sketch.id);
  } catch (error) {
    toast(error.message, "error");
  }
}

/** Asks a signed-out visitor how to save: "sign-in", "local", or null. */
function chooseSignedOutSave() {
  const dialog = $("save-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  return new Promise((resolve) =>
    dialog.addEventListener("close", () => resolve(dialog.returnValue || null), { once: true }),
  );
}

/** Saves a copy of the open document as a new sketch of the visitor's (in
 * this browser when signed out). */
async function fork() {
  const title =
    doc.kind === "sketch" ? `Fork of ${doc.title}` : doc.kind === "example" ? doc.title : doc.title;
  if (!user) {
    const local = createLocalSketch(doc.files, doc.entry, title);
    fillMenu();
    openDoc(localDoc(local.id, catalog.newSketch), { run: true });
    toast("Saved a copy in this browser. Sign in to save sketches to an account.");
    return;
  }
  try {
    const sketch =
      doc.kind === "sketch"
        ? await api.forkSketch(doc.id, { title, ...doc.forServer() })
        : await api.createSketch({ title, ...doc.forServer() });
    if (doc.kind === "sketch" || doc.kind === "example") doc.revert();
    page.sketch = sketch;
    openDoc(sketchDoc(sketch).doc, { run: true });
    toast("Saved your copy. It's private until you change its visibility in About.");
    refreshRemoteSketches();
    refreshThumbnail(sketch.id);
  } catch (error) {
    toast(error.message, "error");
  }
}

/** Moves a sketch kept in this browser to the visitor's account. */
async function saveToAccount() {
  if (doc.kind !== "local") return;
  if (!user) return signIn();
  try {
    const local = doc;
    const sketch = await api.createSketch({ title: local.title, ...local.forServer() });
    deleteLocalSketch(local.id);
    page.sketch = sketch;
    openDoc(sketchDoc(sketch).doc, { run: true });
    fillMenu();
    refreshRemoteSketches();
    refreshThumbnail(sketch.id);
    toast("Saved to your account. It's private until you change its visibility in About.");
  } catch (error) {
    toast(error.message, "error");
  }
}

async function renameDoc() {
  if (!(doc.kind === "local" || doc.canSave)) return;
  const title = await promptText({
    title: "Rename sketch",
    label: "Name",
    value: doc.title,
    confirm: "Rename",
    validate: (value) => (value ? null : "A sketch needs a name."),
  });
  if (!title || title === doc.title) return;
  if (doc.kind === "local") {
    renameLocalSketch(doc.id, title);
  } else {
    try {
      const sketch = await api.updateSketch(doc.id, { title });
      doc.sketch = { ...doc.sketch, title: sketch.title, updated_at: sketch.updated_at };
      // The draft (if any) was made from the version before the rename.
      doc.persist();
      refreshRemoteSketches();
    } catch (error) {
      toast(error.message, "error");
      return;
    }
  }
  doc.title = title;
  menu.setCurrent(doc.title, { kind: doc.kind, id: doc.id });
  document.title = `${doc.title} — easl`;
  fillMenu();
  social.show(doc);
}

async function deleteDoc() {
  if (doc.kind === "local") {
    if (!confirm(`Delete “${doc.title}” from this browser? This can't be undone.`)) return;
    deleteLocalSketch(doc.id);
    fillMenu();
    openTarget({ kind: "example", id: catalog.default ?? examples[0].id });
    return;
  }
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
  const message =
    doc.kind === "example"
      ? "Discard your changes to this example?"
      : "Discard the changes you haven't saved?";
  if (!confirm(message)) return;
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
$("rename").addEventListener("click", () => renameDoc());
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
  saveToAccount: () => saveToAccount(),
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
fillMenu();
refreshRemoteSketches();
if (!supportsWebGPU) {
  document.body.classList.add("no-webgpu");
  $("run").disabled = true;
}
runner.setState("stopped");

function initialDoc() {
  if (page.mode === "sketch" && page.sketch) {
    const { doc: opened, staleDraft } = sketchDoc(page.sketch);
    return { doc: opened, staleDraft };
  }
  if (page.mode === "new") return { doc: newDoc(catalog.newSketch) };
  if (linkedExample) return { doc: exampleDoc(linkedExample) };
  const params = new URLSearchParams(location.search);
  const local = params.get("local");
  if (local) {
    const opened = localDoc(local, catalog.newSketch);
    if (opened) return { doc: opened };
  }
  const last = storage.get("last");
  if (!page.example && last?.startsWith("local:")) {
    const opened = localDoc(last.slice("local:".length), catalog.newSketch);
    if (opened) return { doc: opened };
  }
  for (const id of [page.example, last, catalog.default]) {
    if (id && examplesById.has(id)) return { doc: exampleDoc(examplesById.get(id)) };
  }
  return { doc: exampleDoc(examples[0]) };
}

const initial = initialDoc();
openDoc(initial.doc, { run: true });
if (initial.staleDraft) offerStaleDraft(initial.staleDraft);
editor.focus();

// Signed-out visitors' unsaved new sketches come back after signing in;
// say so where it matters.
if (page.mode === "new" && user && doc.isDirty()) {
  toast("Your unsaved sketch is still here. Save it to your account with ⌘S.");
}

