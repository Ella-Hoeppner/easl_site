// The easl playground: a Monaco editor with easl's language support, and a
// canvas running the program.
//
// - Language support (diagnostics, hover, go-to-definition, completion,
//   formatting, semantic tokens, structural editing) comes from the
//   analysis worker (analysis-worker.js), which runs easl_lsp's code
//   compiled to WebAssembly. A request that takes too long, or a compiler
//   panic, restarts the worker, so the editor never freezes.
// - Each run happens in a fresh iframe (runner.html), so stopping or
//   rerunning a program frees everything it held. A program only runs once
//   the analysis has finished without errors, so a program that would hang
//   the compiler never reaches the runtime, which compiles on this page's
//   thread.
// - Each example's edits are kept in localStorage until it's reset.
// - Sketches are the visitor's own programs, started from the example menu
//   (holding a small starter program) and kept in localStorage.

import {
  registerEasl,
  currentTheme,
  indentAfter,
  callHeadBefore,
} from "./easl-language.js?v=__VERSION__";

const $ = (id) => document.getElementById(id);

const {
  default: defaultExample,
  examples,
  newSketch: NEW_SKETCH,
} = JSON.parse(
  $("examples").textContent,
);

// A program linked from the docs (`#code=<base64url>`) is one more example.
// Its id comes from its text, so reopening the same link restores its
// edits.
const linkedExample = (() => {
  const encoded = new URLSearchParams(location.hash.slice(1)).get("code");
  if (!encoded) return null;
  try {
    const base64 = encoded.replace(/-/g, "+").replace(/_/g, "/");
    const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
    const source = new TextDecoder().decode(bytes);
    return {
      id: `linked-${hash(source)}`,
      title: "The program you opened",
      source,
      linked: true,
    };
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

const MONACO = document.documentElement.dataset.monaco;
const IS_MAC = /Mac|iPhone|iPad/.test(navigator.platform);

// How long an analysis may take before the worker is presumed stuck.
const ANALYSIS_TIMEOUT = 10_000;
// How long other requests may take.
const REQUEST_TIMEOUT = 5_000;
// How long typing must pause before the program is analyzed.
const ANALYSIS_DELAY = 250;
const MAX_CONSOLE_LINES = 1000;

// ---- storage ----

const STORAGE_PREFIX = "easl-playground:";

const storage = {
  get(key) {
    try {
      return localStorage.getItem(STORAGE_PREFIX + key);
    } catch {
      return null;
    }
  },
  set(key, value) {
    try {
      localStorage.setItem(STORAGE_PREFIX + key, value);
    } catch {
      // Storage may be unavailable (private windows, blocked site data).
    }
  },
  remove(key) {
    try {
      localStorage.removeItem(STORAGE_PREFIX + key);
    } catch {}
  },
};

// ---- sketches ----

// The sketches, in the order they were made, as `{id, title}` (stored as
// "sketches"); each one's text is stored as "sketch:<id>". In the page,
// each is an example with `sketch: true`.
const sketches = (() => {
  try {
    const stored = JSON.parse(storage.get("sketches") ?? "[]");
    return stored.map(({ id, title }) => ({
      id,
      title,
      source: NEW_SKETCH,
      sketch: true,
    }));
  } catch {
    return [];
  }
})();
for (const sketch of sketches) examplesById.set(sketch.id, sketch);

function saveSketchList() {
  storage.set(
    "sketches",
    JSON.stringify(sketches.map(({ id, title }) => ({ id, title }))),
  );
}

/** Makes a new sketch holding the starter program, numbered one past the
 * highest so far. */
function newSketch() {
  const number =
    Math.max(0, ...sketches.map((s) => Number(s.id.split("-")[1]) || 0)) + 1;
  const sketch = {
    id: `sketch-${number}`,
    title: `Sketch ${number}`,
    source: NEW_SKETCH,
    sketch: true,
  };
  storage.set(`sketch:${sketch.id}`, NEW_SKETCH);
  sketches.push(sketch);
  examplesById.set(sketch.id, sketch);
  saveSketchList();
  fillSketchMenu();
  return sketch;
}

function renameSketch(sketch, title) {
  sketch.title = title;
  saveSketchList();
  fillSketchMenu();
  showExample(sketch.id);
}

// ---- the analysis worker ----

class AnalysisError extends Error {}

class Analysis {
  constructor() {
    this.nextId = 0;
    this.start();
  }

  start() {
    this.worker = new Worker(
      new URL("analysis-worker.js?v=__VERSION__", import.meta.url),
      { type: "module" },
    );
    this.pending = new Map();
    this.panic = null;
    this.worker.onmessage = ({ data }) => {
      if ("panic" in data) {
        this.panic = data.panic;
        return;
      }
      const request = this.pending.get(data.id);
      if (!request) return;
      this.pending.delete(data.id);
      clearTimeout(request.timer);
      if ("error" in data) {
        const message = data.dead
          ? `internal compiler error: ${this.panic ?? data.error}`
          : data.error;
        request.reject(new AnalysisError(message));
        if (data.dead) this.restart(message);
      } else {
        request.resolve(data.result);
      }
    };
    this.worker.onerror = (event) => {
      event.preventDefault();
      this.restart(`the analysis worker failed: ${event.message}`);
    };
  }

  /** Replaces the worker, failing whatever it was still working on. */
  restart(reason) {
    this.worker.terminate();
    const pending = this.pending;
    this.start();
    for (const request of pending.values()) {
      clearTimeout(request.timer);
      request.reject(new AnalysisError(reason));
    }
  }

  request(method, args = [], timeout = REQUEST_TIMEOUT) {
    return new Promise((resolve, reject) => {
      const id = this.nextId++;
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new AnalysisError("timeout"));
        this.restart("the analysis worker was restarted");
      }, timeout);
      this.pending.set(id, { resolve, reject, timer });
      this.worker.postMessage({ id, method, args });
    });
  }
}

const analysis = new Analysis();

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

// [line, character, line, character] (zero-based, as the worker speaks)
// to a Monaco range, and back.
const toRange = ([sl, sc, el, ec]) =>
  new monaco.Range(sl + 1, sc + 1, el + 1, ec + 1);
const fromSelection = (selection) => [
  selection.startLineNumber - 1,
  selection.startColumn - 1,
  selection.endLineNumber - 1,
  selection.endColumn - 1,
];

// ---- examples and their models ----

const models = new Map();

/** The model holding `id`'s text: its saved draft, or its source. */
function modelFor(id) {
  let model = models.get(id);
  if (!model) {
    const example = examplesById.get(id);
    const text =
      storage.get(example.sketch ? `sketch:${id}` : `draft:${id}`) ??
      example.source;
    model = monaco.editor.createModel(
      text,
      "easl",
      monaco.Uri.parse(`easl:/${id}.easl`),
    );
    model.exampleId = id;
    let saveTimer;
    model.onDidChangeContent(() => {
      scheduleAnalysis(model);
      clearTimeout(saveTimer);
      saveTimer = setTimeout(() => saveDraft(model), 400);
      updateEditedState();
    });
    models.set(id, model);
  }
  return model;
}

function saveDraft(model) {
  const id = model.exampleId;
  if (examplesById.get(id).sketch) {
    storage.set(`sketch:${id}`, model.getValue());
  } else if (model.getValue() === examplesById.get(id).source) {
    storage.remove(`draft:${id}`);
  } else {
    storage.set(`draft:${id}`, model.getValue());
  }
}

function isEdited(model) {
  return model.getValue() !== examplesById.get(model.exampleId).source;
}

function updateEditedState() {
  const model = editor.getModel();
  $("reset").disabled = !isEdited(model);
}

/** The bar's buttons for `example`: a sketch can be renamed, and has no
 * original to reset to. */
function updateBarButtons(example) {
  $("rename").hidden = !example.sketch;
  $("reset").hidden = !!example.sketch;
}

// The example menu lists the examples in order, except that the examples
// sharing a `menu` are listed in a submenu of that name, where the first of
// them would be. A submenu opens on hover, on a click (for touch screens),
// or with the right arrow key. Below a line come the sketches and the item
// that makes a new one.

// The example in the editor.
let shownExample = null;

const picker = $("example-picker");
const menuButton = $("example");
const menu = $("example-menu");

function fillExampleMenu() {
  const submenus = new Map();
  for (const example of examples) {
    const item = menuItem(example.title);
    item.firstChild.dataset.example = example.id;
    if (!example.menu) {
      menu.append(item);
      continue;
    }
    let submenu = submenus.get(example.menu);
    if (!submenu) {
      const parent = menuItem(example.menu);
      parent.classList.add("has-submenu");
      parent.firstChild.setAttribute("aria-haspopup", "menu");
      submenu = document.createElement("ul");
      submenu.className = "menu submenu";
      submenu.role = "menu";
      submenu.ariaLabel = example.menu;
      parent.append(submenu);
      menu.append(parent);
      submenus.set(example.menu, submenu);
    }
    submenu.append(item);
  }
  const separator = document.createElement("li");
  separator.role = "separator";
  separator.className = "menu-separator";
  const newItem = menuItem("New sketch…");
  newItem.firstChild.dataset.action = "new-sketch";
  newItem.classList.add("new-sketch");
  menu.append(separator, newItem);
  fillSketchMenu();
}

/** Lists the sketches between the menu's line and its "New sketch…". */
function fillSketchMenu() {
  for (const item of menu.querySelectorAll(".sketch-item")) item.remove();
  const newItem = menu.querySelector(".new-sketch");
  for (const sketch of sketches) {
    const item = menuItem(sketch.title);
    item.classList.add("sketch-item");
    item.firstChild.dataset.example = sketch.id;
    item.firstChild.classList.toggle("current", sketch.id === shownExample);
    newItem.before(item);
  }
}

function menuItem(label) {
  const item = document.createElement("li");
  item.role = "none";
  const button = document.createElement("button");
  button.role = "menuitem";
  button.textContent = label;
  item.append(button);
  return item;
}

/** How the menu's button names `example`. */
function exampleLabel(example) {
  return example.menu ? `${example.menu} › ${example.title}` : example.title;
}

function openMenu() {
  menu.hidden = false;
  menuButton.ariaExpanded = "true";
  const current = menu.querySelector("[data-example].current");
  const submenu = current?.closest(".has-submenu");
  submenu?.classList.add("open");
  (current ?? menu.querySelector("[role=menuitem]")).focus();
}

function closeMenu() {
  menu.hidden = true;
  menuButton.ariaExpanded = "false";
  closeSubmenus();
}

function closeSubmenus(except) {
  for (const parent of menu.querySelectorAll(".has-submenu.open")) {
    if (parent !== except) parent.classList.remove("open");
  }
}

/** The menu items `item` is listed beside (in its own menu). */
function siblingItems(item) {
  return [...item.closest("[role=menu]").children]
    .map((child) => child.firstChild)
    .filter((child) => child?.role === "menuitem");
}

menuButton.addEventListener("click", () => {
  if (menu.hidden) openMenu();
  else closeMenu();
});

menuButton.addEventListener("keydown", (event) => {
  if (event.key === "ArrowDown" && menu.hidden) {
    event.preventDefault();
    openMenu();
  }
});

menu.addEventListener("click", (event) => {
  const button = event.target.closest("button");
  if (!button) return;
  const parent = button.parentElement;
  if (parent.classList.contains("has-submenu")) {
    closeSubmenus(parent);
    parent.classList.toggle("open");
    return;
  }
  closeMenu();
  menuButton.focus();
  if (button.dataset.action === "new-sketch") {
    showExample(newSketch().id);
    editor.focus();
  } else if (button.dataset.example !== editor.getModel().exampleId) {
    showExample(button.dataset.example);
    run();
  }
});

// Hovering one top-level item closes a submenu another was holding open.
menu.addEventListener("pointerover", (event) => {
  const item = event.target.closest("#example-menu > li");
  if (item) closeSubmenus(item);
});

menu.addEventListener("keydown", (event) => {
  const item = event.target.closest("[role=menuitem]");
  if (!item) return;
  const parent = item.parentElement;
  const items = siblingItems(item);
  const index = items.indexOf(item);
  const inSubmenu = parent.parentElement !== menu;
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    const step = event.key === "ArrowDown" ? 1 : -1;
    items[(index + step + items.length) % items.length].focus();
  } else if (
    event.key === "ArrowRight" &&
    parent.classList.contains("has-submenu")
  ) {
    parent.classList.add("open");
    parent.querySelector(".submenu [role=menuitem]").focus();
  } else if (event.key === "ArrowLeft" && inSubmenu) {
    const opener = parent.closest(".has-submenu");
    opener.classList.remove("open");
    opener.firstChild.focus();
  } else if (event.key === "Escape") {
    closeMenu();
    menuButton.focus();
  } else if (event.key === "Tab") {
    closeMenu();
  } else {
    return;
  }
  event.preventDefault();
});

document.addEventListener("pointerdown", (event) => {
  if (!menu.hidden && !picker.contains(event.target)) closeMenu();
});

function initialExample() {
  if (linkedExample) return linkedExample.id;
  const requested = new URLSearchParams(location.search).get("example");
  for (const id of [requested, storage.get("last"), defaultExample]) {
    if (id && examplesById.has(id)) return id;
  }
  return examples[0].id;
}

function showExample(id) {
  shownExample = id;
  const model = modelFor(id);
  editor.setModel(model);
  menuButton.textContent = exampleLabel(examplesById.get(id));
  for (const item of menu.querySelectorAll("[data-example]")) {
    item.classList.toggle("current", item.dataset.example === id);
  }
  for (const parent of menu.querySelectorAll(".has-submenu")) {
    parent.firstChild.classList.toggle(
      "current",
      parent.querySelector("[data-example].current") !== null,
    );
  }
  const url = new URL(location.href);
  if (examplesById.get(id).linked) {
    url.searchParams.delete("example");
  } else {
    storage.set("last", id);
    url.searchParams.set("example", id);
    url.hash = "";
  }
  history.replaceState(null, "", url);
  updateBarButtons(examplesById.get(id));
  updateEditedState();
  scheduleAnalysis(model, 0);
}

// ---- the editor ----

fillExampleMenu();

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

// ---- diagnostics ----

// The last analysis: which model and version it describes, and how it
// went. Hover and go-to-definition answer only for that text.
let analyzed = { model: null, version: -1, diagnostics: [], failure: null };
let analysisTimer;
let analysisRun = null;

function scheduleAnalysis(model, delay = ANALYSIS_DELAY) {
  clearTimeout(analysisTimer);
  setStatus("analyzing", "Checking…");
  analysisTimer = setTimeout(() => analyze(model), delay);
}

/** Analyzes `model`'s current text, unless that's already been done. */
function analyze(model) {
  clearTimeout(analysisTimer);
  const version = model.getVersionId();
  if (analyzed.model === model && analyzed.version === version) {
    return Promise.resolve(analyzed);
  }
  if (analysisRun?.model === model && analysisRun.version === version) {
    return analysisRun.promise;
  }
  const text = model.getValue();
  const promise = analysis
    .request("analyze", [text], ANALYSIS_TIMEOUT)
    .then(
      (diagnostics) => ({ diagnostics, failure: null }),
      (error) => ({ diagnostics: [], failure: failureMessage(error, text) }),
    )
    .then(({ diagnostics, failure }) => {
      const result = { model, version, diagnostics, failure };
      if (model.getVersionId() === version) {
        analyzed = result;
        showDiagnostics(result);
      }
      return result;
    });
  analysisRun = { model, version, promise };
  return promise;
}

/**
 * What to say when the compiler couldn't check `text`: it took too long,
 * or it crashed. Both currently happen on recursive functions (which easl
 * doesn't allow, but the compiler doesn't yet reject cleanly), so those are
 * named when `text` has any.
 */
function failureMessage(error, text) {
  const recursive = recursiveFunctions(text);
  const hint =
    recursive.length > 0
      ? ` easl doesn't allow recursion, and ${recursive
          .map((name) => `\`${name}\``)
          .join(", ")} ${recursive.length === 1 ? "appears to call itself" : "appear to call themselves"}.`
      : "";
  if (error.message === "timeout") {
    return `The compiler didn't finish checking this program.${hint}`;
  }
  if (error.message.startsWith("internal compiler error")) {
    const detail = error.message.includes("RuntimeError")
      ? ""
      : ` (${error.message.replace(/^internal compiler error: /, "")})`;
    return `The compiler crashed while checking this program${detail}.${hint}`;
  }
  return error.message;
}

/** The functions defined in `text` that mention their own name. */
function recursiveFunctions(text) {
  const names = [];
  const definition = /\(defn\s+\(?([^\s()\[\]{}]+)/g;
  for (const match of text.matchAll(definition)) {
    const body = text.slice(match.index + match[0].length, formEnd(text, match.index));
    const name = match[1];
    const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    if (new RegExp(`[\\s(\\[{]${escaped}[\\s)\\]}]`).test(body)) names.push(name);
  }
  return names;
}

/** The index just past the form opening at `start`. */
function formEnd(text, start) {
  let depth = 0;
  for (let i = start; i < text.length; i++) {
    if ("([{".includes(text[i])) depth++;
    else if (")]}".includes(text[i]) && --depth === 0) return i + 1;
  }
  return text.length;
}

function showDiagnostics({ model, diagnostics, failure }) {
  const markers = diagnostics.map((diagnostic) => {
    const range = toRange(diagnostic.range);
    return {
      severity: monaco.MarkerSeverity.Error,
      message: diagnostic.message,
      ...range,
      // A zero-width range (an error with no exact place) still shows.
      endColumn: range.isEmpty() ? range.endColumn + 1 : range.endColumn,
      relatedInformation: diagnostic.related.map((related) => ({
        resource: model.uri,
        message: "related location",
        ...toRange(related),
      })),
    };
  });
  if (failure) {
    markers.push({
      severity: monaco.MarkerSeverity.Warning,
      message: failure,
      startLineNumber: 1,
      startColumn: 1,
      endLineNumber: 1,
      endColumn: 2,
    });
  }
  monaco.editor.setModelMarkers(model, "easl", markers);
  if (model !== editor.getModel()) return;
  if (failure) {
    setStatus("failure", "Couldn't check");
  } else if (diagnostics.length === 0) {
    setStatus("ok", "No problems");
  } else {
    const count = diagnostics.length;
    setStatus("errors", `${count} problem${count === 1 ? "" : "s"}`);
  }
}

function setStatus(kind, text) {
  const status = $("status");
  status.dataset.kind = kind;
  status.textContent = text;
}

$("status").addEventListener("click", () => {
  if (analyzed.diagnostics.length > 0) {
    editor.focus();
    editor.trigger("status", "editor.action.marker.next", null);
  }
});

// ---- language features ----

/** Whether the worker's last analysis describes `model` as it is now. */
const analyzedNow = (model) =>
  analyzed.model === model && analyzed.version === model.getVersionId();

const quietly = (promise) => promise.catch(() => null);

monaco.languages.registerHoverProvider("easl", {
  async provideHover(model, position) {
    if (!analyzedNow(model)) return null;
    const hover = await quietly(
      analysis.request("hover", [position.lineNumber - 1, position.column - 1]),
    );
    if (!hover || !analyzedNow(model)) return null;
    return {
      range: toRange(hover.range),
      contents: hover.contents.map((value) => ({ value })),
    };
  },
});

monaco.languages.registerDefinitionProvider("easl", {
  async provideDefinition(model, position) {
    if (!analyzedNow(model)) return null;
    const ranges = await quietly(
      analysis.request("definition", [
        position.lineNumber - 1,
        position.column - 1,
      ]),
    );
    if (!ranges || !analyzedNow(model)) return null;
    return ranges.map((range) => ({ uri: model.uri, range: toRange(range) }));
  },
});

// Inside a call, the called function's declarations (each overload's).
monaco.languages.registerSignatureHelpProvider("easl", {
  signatureHelpTriggerCharacters: [" ", "("],
  signatureHelpRetriggerCharacters: [" "],
  async provideSignatureHelp(model, position) {
    if (!analyzedNow(model)) return null;
    const before = model.getValueInRange(
      new monaco.Range(1, 1, position.lineNumber, position.column),
    );
    const head = callHeadBefore(before);
    if (head === null) return null;
    const headPosition = model.getPositionAt(head);
    const hover = await quietly(
      analysis.request("hover", [
        headPosition.lineNumber - 1,
        headPosition.column - 1,
      ]),
    );
    const declarations = (hover?.contents ?? [])
      .map((content) => content.replace(/^```easl\n|\n```$/g, ""))
      .filter((declaration) => declaration.startsWith("("));
    if (declarations.length === 0 || !analyzedNow(model)) return null;
    return {
      value: {
        signatures: declarations.map((label) => ({ label, parameters: [] })),
        activeSignature: 0,
        activeParameter: 0,
      },
      dispose() {},
    };
  },
});

// LSP completion item kinds, by number, to Monaco's.
const COMPLETION_KINDS = {
  3: "Function",
  6: "Variable",
  9: "Module",
  13: "Enum",
  14: "Keyword",
  20: "EnumMember",
  21: "Constant",
  22: "Struct",
};

monaco.languages.registerCompletionItemProvider("easl", {
  triggerCharacters: ["-", "/"],
  async provideCompletionItems(model, position) {
    const items = await quietly(
      analysis.request("completions", [
        model.getValue(),
        position.lineNumber - 1,
        position.column - 1,
      ]),
    );
    return {
      suggestions: (items ?? []).map((item) => ({
        label: item.label,
        kind: monaco.languages.CompletionItemKind[
          COMPLETION_KINDS[item.kind] ?? "Text"
        ],
        sortText: item.sortText,
        insertText: item.label,
        range: toRange(item.range),
      })),
    };
  },
});

monaco.languages.registerDocumentFormattingEditProvider("easl", {
  async provideDocumentFormattingEdits(model) {
    const formatted = await quietly(
      analysis.request("format", [model.getValue()]),
    );
    if (formatted === null || formatted === model.getValue()) return [];
    return [{ range: model.getFullModelRange(), text: formatted }];
  },
});

monaco.languages.registerSelectionRangeProvider("easl", {
  async provideSelectionRanges(model, positions) {
    const text = model.getValue();
    return Promise.all(
      positions.map(async (position) => {
        const ranges = await quietly(
          analysis.request("selectionRanges", [
            text,
            position.lineNumber - 1,
            position.column - 1,
          ]),
        );
        return (ranges ?? []).map((range) => ({ range: toRange(range) }));
      }),
    );
  },
});

// The semantic tokens color what the tokenizer can't, like a form commented
// out with `#_`. Their bracket types (by nesting depth) are left to the
// editor's own bracket colors.
const tokenTypes = await quietly(analysis.request("semanticTokenTypes"));
if (tokenTypes) {
  monaco.languages.registerDocumentSemanticTokensProvider("easl", {
    getLegend: () => ({ tokenTypes, tokenModifiers: [] }),
    async provideDocumentSemanticTokens(model) {
      const data = await quietly(
        analysis.request("semanticTokens", [model.getValue()]),
      );
      return data ? { data: new Uint32Array(data) } : null;
    },
    releaseDocumentSemanticTokens() {},
  });
}

// ---- editing commands ----

const KeyMod = monaco.KeyMod;
const KeyCode = monaco.KeyCode;
// Control on every platform (on a Mac, Cmd belongs to the browser).
const CTRL = IS_MAC ? KeyMod.WinCtrl : KeyMod.CtrlCmd;

/** Runs one of easl_lsp's structural commands on the selection. */
async function structural(command) {
  const model = editor.getModel();
  const selection = editor.getSelection();
  const version = model.getVersionId();
  const result = await quietly(
    analysis.request("structural", [
      command,
      model.getValue(),
      ...fromSelection(selection),
    ]),
  );
  if (!result || model.getVersionId() !== version) return;
  if (command === "moveFormForward" || command === "moveFormBackward") {
    editor.pushUndoStop();
    editor.executeEdits("easl", [
      { range: toRange(result.range), text: result.newText },
    ]);
    editor.pushUndoStop();
    editor.setSelection(toRange(result.selection));
  } else {
    const [line, character] = result;
    editor.setPosition({ lineNumber: line + 1, column: character + 1 });
  }
  editor.revealPosition(editor.getPosition());
}

// On a Mac, the browser's shortcuts use ⌘, so the Control bindings
// easl_lsp's VS Code extension uses are free too (Windows and Linux browsers
// reserve Ctrl+W, and Ctrl+A and Ctrl+D mean select-all and bookmark).
const macOnly = (...keybindings) => (IS_MAC ? keybindings : []);

const actions = [
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
    label: "Save: format (if Format on save is on) and run",
    keybindings: [KeyMod.CtrlCmd | KeyCode.KeyS],
    run: () => save(),
  },
  {
    id: "easl.moveFormForward",
    label: "Easl: Move Form Forward",
    keybindings: [
      CTRL | KeyMod.Alt | KeyCode.RightArrow,
      ...macOnly(KeyMod.WinCtrl | KeyMod.Shift | KeyCode.KeyD),
    ],
    run: () => structural("moveFormForward"),
  },
  {
    id: "easl.moveFormBackward",
    label: "Easl: Move Form Backward",
    keybindings: [
      CTRL | KeyMod.Alt | KeyCode.LeftArrow,
      ...macOnly(KeyMod.WinCtrl | KeyMod.Shift | KeyCode.KeyA),
    ],
    run: () => structural("moveFormBackward"),
  },
  {
    id: "easl.moveCursorToStart",
    label: "Easl: Move Cursor to Start of Current Form",
    keybindings: [
      CTRL | KeyMod.Alt | KeyCode.UpArrow,
      ...macOnly(KeyMod.WinCtrl | KeyCode.KeyA),
    ],
    run: () => structural("moveCursorToStart"),
  },
  {
    id: "easl.moveCursorToEnd",
    label: "Easl: Move Cursor to End of Current Form",
    keybindings: [
      CTRL | KeyMod.Alt | KeyCode.DownArrow,
      ...macOnly(KeyMod.WinCtrl | KeyCode.KeyD),
    ],
    run: () => structural("moveCursorToEnd"),
  },
];
for (const action of actions) editor.addAction(action);

// Enter indents the new line by its position in the forms around it: two
// past an unclosed `(`, one past an unclosed `[` or `{`.
editor.addCommand(
  KeyCode.Enter,
  () => {
    const model = editor.getModel();
    const selections = editor.getSelections();
    const edits = selections.map((selection) => {
      const before = model.getValueInRange(
        new monaco.Range(
          1,
          1,
          selection.startLineNumber,
          selection.startColumn,
        ),
      );
      return {
        range: selection,
        text: "\n" + " ".repeat(indentAfter(before)),
        forceMoveMarkers: true,
      };
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

// ---- running ----

const supportsWebGPU = Boolean(navigator.gpu);
let runner = null;
let runToken = 0;

/**
 * Adds a line to the console. A problem's line number (`line`, one-based)
 * makes the line a link to it.
 */
function log(kind, text, line = null) {
  const output = $("console");
  const entry = document.createElement("div");
  entry.className = `line ${kind}`;
  if (line !== null) {
    const link = document.createElement("button");
    link.className = "line-link";
    link.textContent = `line ${line}`;
    link.addEventListener("click", () => {
      editor.revealLineInCenter(line);
      editor.setPosition({ lineNumber: line, column: 1 });
      editor.focus();
    });
    entry.append(link, ": ");
  }
  entry.append(text);
  output.append(entry);
  while (output.childElementCount > MAX_CONSOLE_LINES) {
    output.firstElementChild.remove();
  }
  output.scrollTop = output.scrollHeight;
  $("console-empty").hidden = true;
}

function clearConsole() {
  $("console").replaceChildren();
  $("console-empty").hidden = false;
}

function setRunState(state) {
  document.body.dataset.run = state;
  $("pause").textContent = state === "paused" ? "Resume" : "Pause";
  $("pause").disabled = state !== "running" && state !== "paused";
  $("stop").disabled = state !== "running" && state !== "paused";
}

function stop() {
  runToken++;
  runner?.iframe.remove();
  runner = null;
  setRunState("stopped");
}

async function run() {
  if (!supportsWebGPU) return;
  const model = editor.getModel();
  const source = model.getValue();
  const token = ++runToken;
  setRunState("checking");
  const result = await analyze(model);
  if (token !== runToken) return;
  if (result.failure || result.diagnostics.length > 0) {
    runToken++;
    runner?.iframe.remove();
    runner = null;
    setRunState("stopped");
    clearConsole();
    if (result.failure) {
      log("error", result.failure);
    } else {
      for (const diagnostic of result.diagnostics) {
        log("error", diagnostic.message, diagnostic.range[0] + 1);
      }
      editor.revealRangeInCenter(toRange(result.diagnostics[0].range));
    }
    return;
  }
  runner?.iframe.remove();
  runner = null;
  clearConsole();
  setRunState("starting");
  const iframe = document.createElement("iframe");
  iframe.title = "The program's output";
  iframe.allow = "midi; autoplay";
  iframe.src = new URL("runner.html?v=__VERSION__", import.meta.url).href;
  $("stage").append(iframe);
  await new Promise((resolve) =>
    iframe.addEventListener("load", resolve, { once: true }),
  );
  if (token !== runToken) return;
  let program;
  try {
    program = await iframe.contentWindow.runEasl(source, (line) => {
      if (token === runToken) log("print", line);
    });
  } catch (error) {
    if (token !== runToken) return;
    log("error", String(error?.message ?? error));
    setRunState("stopped");
    return;
  }
  if (token !== runToken) return;
  runner = { iframe, program };
  setRunState("running");
  program.finished.then(
    () => {
      if (token !== runToken) return;
      log("info", "The program finished.");
      setRunState("finished");
    },
    (error) => {
      if (token !== runToken) return;
      log("error", String(error?.message ?? error).replace(/^easl: /, ""));
      setRunState("stopped");
    },
  );
}

$("run").addEventListener("click", () => run());
$("stop").addEventListener("click", () => stop());
$("pause").addEventListener("click", () => {
  if (!runner) return;
  if (runner.program.paused) {
    runner.program.resume();
    setRunState("running");
  } else {
    runner.program.pause();
    setRunState("paused");
  }
});
$("clear").addEventListener("click", () => clearConsole());
$("format").addEventListener("click", () => {
  editor.focus();
  editor.getAction("editor.action.formatDocument").run();
});
$("reset").addEventListener("click", () => {
  const model = editor.getModel();
  if (!isEdited(model)) return;
  if (!confirm("Discard your changes to this example?")) return;
  model.pushEditOperations(
    [],
    [{ range: model.getFullModelRange(), text: examplesById.get(model.exampleId).source }],
    () => null,
  );
  storage.remove(`draft:${model.exampleId}`);
  updateEditedState();
});
// Ctrl/Cmd+Enter runs and Ctrl/Cmd+S saves from anywhere on the page, not
// just the editor.
document.addEventListener("keydown", (event) => {
  if (event.defaultPrevented || !(event.ctrlKey || event.metaKey)) return;
  if (event.key === "Enter") {
    event.preventDefault();
    run();
  } else if (event.key === "s") {
    event.preventDefault();
    save();
  }
});

// ---- format on save ----

const formatOnSave = $("format-on-save");
formatOnSave.checked = storage.get("format-on-save") !== "false";
formatOnSave.addEventListener("change", () =>
  storage.set("format-on-save", String(formatOnSave.checked)),
);

/**
 * Saving: formats the program when Format on save is on, then runs it. The
 * editor's format command applies the smallest edits it can, so the cursor
 * stays put, and one undo reverts the formatting. A program that doesn't
 * parse is left as it is.
 */
async function save() {
  if (formatOnSave.checked) {
    await editor.getAction("editor.action.formatDocument").run();
  }
  run();
}

// ---- the help dialog ----

const key = (mac, other) => (IS_MAC ? mac : other);
const SHORTCUTS = [
  ["Run", key("⌘ Enter", "Ctrl+Enter")],
  ["Save: format (if Format on save is on) and run", key("⌘ S", "Ctrl+S")],
  ["Format", key("⇧ ⌥ F", "Shift+Alt+F")],
  ["Expand selection to the enclosing form", key("⌃ W or ⌃ ⇧ ⌘ →", "Shift+Alt+→")],
  ["Shrink selection", key("⌃ ⇧ ⌘ ←", "Shift+Alt+←")],
  ["Cursor to start / end of form", key("⌃ A / ⌃ D, or ⌃ ⌥ ↑ / ↓", "Ctrl+Alt+↑ / ↓")],
  ["Move form backward / forward", key("⌃ ⇧ A / ⌃ ⇧ D, or ⌃ ⌥ ← / →", "Ctrl+Alt+← / →")],
  ["Go to definition", key("F12 or ⌘ click", "F12 or Ctrl+click")],
  ["Next problem", key("F8", "F8")],
  ["Suggestions", key("⌃ Space", "Ctrl+Space")],
  ["Toggle comment", key("⌘ /", "Ctrl+/")],
  ["All commands", "F1"],
];
$("shortcuts").replaceChildren(
  ...SHORTCUTS.map(([action, keys]) => {
    const row = document.createElement("tr");
    const name = document.createElement("td");
    name.textContent = action;
    const binding = document.createElement("td");
    const kbd = document.createElement("kbd");
    kbd.textContent = keys;
    binding.append(kbd);
    row.append(name, binding);
    return row;
  }),
);
$("help").addEventListener("click", () => $("help-dialog").showModal());

// ---- renaming a sketch ----

$("rename").addEventListener("click", () => {
  const sketch = examplesById.get(editor.getModel().exampleId);
  if (!sketch.sketch) return;
  const input = $("rename-input");
  input.value = sketch.title;
  $("rename-dialog").showModal();
  input.select();
});
// Enter renames (the form's first button, which Enter would press, is
// Cancel).
$("rename-input").addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    $("rename-dialog").close("rename");
  }
});
$("rename-dialog").addEventListener("close", () => {
  const dialog = $("rename-dialog");
  const title = $("rename-input").value.trim();
  const sketch = examplesById.get(editor.getModel().exampleId);
  if (dialog.returnValue === "rename" && title && sketch.sketch) {
    renameSketch(sketch, title);
  }
  dialog.returnValue = "";
});
$("help-close").addEventListener("click", () => $("help-dialog").close());

// ---- the splitters ----

/**
 * Makes `splitter` draggable: while it's dragged, `onDrag` gets each
 * pointer position, and the iframe running the program (which would
 * otherwise swallow the pointer) ignores it.
 */
function draggable(splitter, onDrag) {
  splitter.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    splitter.setPointerCapture(event.pointerId);
    splitter.classList.add("active");
    document.body.classList.add("dragging");
    const move = (moveEvent) => onDrag(moveEvent);
    const end = () => {
      splitter.classList.remove("active");
      document.body.classList.remove("dragging");
      splitter.removeEventListener("pointermove", move);
      splitter.removeEventListener("pointerup", end);
      splitter.removeEventListener("pointercancel", end);
    };
    splitter.addEventListener("pointermove", move);
    splitter.addEventListener("pointerup", end);
    splitter.addEventListener("pointercancel", end);
  });
}

/**
 * A layout fraction (a CSS variable on `element`, saved under `key`),
 * restored from a previous visit when it's within `min`..`max`. Returns
 * the setter that clamps, applies, and saves it.
 */
function fraction(element, variable, key, min, max) {
  const saved = Number(storage.get(key));
  if (storage.get(key) !== null && saved >= min && saved <= max) {
    element.style.setProperty(variable, saved);
  }
  return (value) => {
    const clamped = Math.min(max, Math.max(min, value));
    element.style.setProperty(variable, clamped);
    storage.set(key, clamped);
  };
}

// Between the editor and the output: beside each other, or stacked on
// small screens.
const panes = $("panes");
const setEditorFraction = fraction(panes, "--editor-fraction", "split", 0.15, 0.85);
draggable($("splitter"), (event) => {
  const box = panes.getBoundingClientRect();
  setEditorFraction(
    matchMedia("(max-width: 52rem)").matches
      ? (event.clientY - box.top) / box.height
      : (event.clientX - box.left) / box.width,
  );
});

// Between the canvas and the console: the console's height, as a fraction
// of the output pane, from collapsed to its bar up to most of the pane.
const output = document.querySelector(".output-pane");
const setConsoleFraction = fraction(output, "--console-fraction", "console-split", 0, 0.85);
draggable($("console-splitter"), (event) => {
  const box = output.getBoundingClientRect();
  const bar = document.querySelector(".console-bar").getBoundingClientRect();
  const splitterHeight = $("console-splitter").getBoundingClientRect().height;
  setConsoleFraction(
    (box.bottom - event.clientY - splitterHeight / 2 - bar.height) / box.height,
  );
});

// ---- start ----

if (!supportsWebGPU) {
  document.body.classList.add("no-webgpu");
  $("run").disabled = true;
}
setRunState("stopped");
showExample(initialExample());
editor.focus();
if (supportsWebGPU) run();
