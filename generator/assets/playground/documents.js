// What the editor edits: a document, made of one or more easl files, of
// one of four kinds:
//
// - "example": one of the docs' examples. Edits are kept in this browser as
//   a draft until it's reset.
// - "local": a sketch kept only in this browser (made without an account).
//   It's saved in localStorage as it's edited.
// - "sketch": a sketch on the server. Its owner's edits are kept as a
//   draft in this browser until they're saved to the server; anyone else
//   can edit it here too (kept as a draft) and fork it to save a copy.
// - "new": a sketch that hasn't been saved anywhere yet, kept as a draft.
//
// Each document also remembers which `@cpu` entry point it runs (`entry`,
// `{file, name}`), since a sketch can have several.

import { storage } from "./util.js?v=__VERSION__";

export const MAIN_FILE = "main.easl";

const sameFiles = (a, b) =>
  a.length === b.length &&
  a.every((file, i) => file.path === b[i].path && file.text === b[i].text);

const sameEntry = (a, b) =>
  (a?.file ?? null) === (b?.file ?? null) && (a?.name ?? null) === (b?.name ?? null);

const copyFiles = (files) => files.map(({ path, text }) => ({ path, text }));

/** A stored `{files, entry}`, or a single file's text from before sketches
 * had several files. */
function readStoredFiles(stored) {
  if (typeof stored === "string") {
    return { files: [{ path: MAIN_FILE, text: stored }], entry: null };
  }
  if (stored && Array.isArray(stored.files) && stored.files.length > 0) {
    return { files: copyFiles(stored.files), entry: stored.entry ?? null };
  }
  return null;
}

function readStorage(key) {
  const raw = storage.get(key);
  if (raw === null) return null;
  try {
    return readStoredFiles(JSON.parse(raw));
  } catch {
    return readStoredFiles(raw);
  }
}

export class Doc {
  constructor({ kind, id = null, title, files, entry = null, baseline, sketch = null, example = null }) {
    this.kind = kind;
    this.id = id;
    this.title = title;
    this.files = copyFiles(files);
    this.entry = entry;
    // What the document is when it has no unsaved changes.
    this.baseline = { files: copyFiles(baseline.files), entry: baseline.entry ?? null };
    // The server's description of a "sketch" document.
    this.sketch = sketch;
    // The example a document of kind "example" is.
    this.example = example;
  }

  /** A key naming the document in this browser. */
  get key() {
    return this.kind === "new" ? "new" : `${this.kind}:${this.id}`;
  }

  /** The address that opens the document. */
  get url() {
    switch (this.kind) {
      case "sketch":
        return `/s/${this.id}`;
      case "example":
        return `/playground?example=${encodeURIComponent(this.id)}`;
      case "local":
        return `/playground?local=${encodeURIComponent(this.id)}`;
      default:
        return "/new";
    }
  }

  get canSave() {
    return this.kind === "sketch" && Boolean(this.sketch?.can_edit);
  }

  file(path) {
    return this.files.find((file) => file.path === path) ?? null;
  }

  setText(path, text) {
    const file = this.file(path);
    if (file) file.text = text;
  }

  /** Whether it differs from what it is with no unsaved changes. */
  isDirty() {
    return !sameFiles(this.files, this.baseline.files) || !sameEntry(this.entry, this.baseline.entry);
  }

  /** Keeps the document's current state in this browser. */
  persist() {
    const state = { files: this.files, entry: this.entry };
    switch (this.kind) {
      case "local":
        storage.setJSON(`sketch:${this.id}`, state);
        break;
      case "sketch":
        if (this.isDirty()) {
          storage.setJSON(`draft:${this.key}`, { ...state, base: this.sketch.updated_at });
        } else {
          storage.remove(`draft:${this.key}`);
        }
        break;
      default:
        if (this.isDirty()) {
          storage.setJSON(`draft:${this.key}`, state);
        } else {
          storage.remove(`draft:${this.key}`);
        }
    }
  }

  /** Returns to the state with no unsaved changes, forgetting the draft. */
  revert() {
    this.files = copyFiles(this.baseline.files);
    this.entry = this.baseline.entry;
    storage.remove(`draft:${this.key}`);
    if (this.kind === "example") storage.remove(`draft:${this.id}`);
  }

  /** Marks the current state as saved (after the server accepted it). */
  markSaved(sketch) {
    this.sketch = sketch;
    this.title = sketch.title;
    this.baseline = { files: copyFiles(this.files), entry: this.entry };
    storage.remove(`draft:${this.key}`);
  }

  /** The files and entry, as the server takes them. */
  forServer() {
    return {
      files: this.files.map(({ path, text }) => ({ path, content: text })),
      entry_file: this.entry?.file ?? null,
      entry_name: this.entry?.name ?? null,
    };
  }
}

// ---- documents of each kind ----

export function exampleDoc(example) {
  const baseline = { files: [{ path: MAIN_FILE, text: example.source }], entry: null };
  const draft =
    readStorage(`draft:example:${example.id}`) ?? readStorage(`draft:${example.id}`);
  return new Doc({
    kind: "example",
    id: example.id,
    title: example.menu ? `${example.menu} › ${example.title}` : example.title,
    files: draft?.files ?? baseline.files,
    entry: draft?.entry ?? null,
    baseline,
    example,
  });
}

/** A server sketch, with this browser's draft of it if there's one. The
 * draft is used only if it was made from the sketch as it is now; one made
 * from an older version comes back separately as `staleDraft`. */
export function sketchDoc(sketch) {
  const baseline = {
    files: sketch.files.map(({ path, content }) => ({ path, text: content })),
    entry: sketch.entry_file ? { file: sketch.entry_file, name: sketch.entry_name } : null,
  };
  const key = `draft:sketch:${sketch.id}`;
  const stored = storage.getJSON(key);
  const draft = readStoredFiles(stored);
  const current = draft && stored.base === sketch.updated_at;
  const doc = new Doc({
    kind: "sketch",
    id: sketch.id,
    title: sketch.title,
    files: current ? draft.files : baseline.files,
    entry: current ? draft.entry : baseline.entry,
    baseline,
    sketch,
  });
  return { doc, staleDraft: draft && !current ? draft : null };
}

export function newDoc(starter) {
  const baseline = { files: [{ path: MAIN_FILE, text: starter }], entry: null };
  const draft = readStorage("draft:new");
  return new Doc({
    kind: "new",
    title: "New sketch",
    files: draft?.files ?? baseline.files,
    entry: draft?.entry ?? null,
    baseline,
  });
}

// ---- sketches kept in this browser ----

/** The sketches kept in this browser, in the order they were made, as
 * `{id, title}`. */
export function localSketches() {
  const list = storage.getJSON("sketches");
  return Array.isArray(list) ? list.filter((s) => s && s.id && s.title) : [];
}

function saveLocalList(list) {
  storage.setJSON("sketches", list);
}

export function localDoc(id, starter) {
  const entry = localSketches().find((sketch) => sketch.id === id);
  if (!entry) return null;
  const stored = readStorage(`sketch:${id}`) ?? {
    files: [{ path: MAIN_FILE, text: starter }],
    entry: null,
  };
  // A local sketch is saved as it's edited, so it never has unsaved changes.
  const doc = new Doc({ kind: "local", id, title: entry.title, ...stored, baseline: stored });
  doc.isDirty = () => false;
  return doc;
}

/** Makes a sketch in this browser holding `files`, numbered one past the
 * highest so far. */
export function createLocalSketch(files, entry = null, title = null) {
  const list = localSketches();
  const number = Math.max(0, ...list.map((s) => Number(s.id.split("-")[1]) || 0)) + 1;
  const sketch = { id: `sketch-${number}`, title: title ?? `Sketch ${number}` };
  storage.setJSON(`sketch:${sketch.id}`, { files: copyFiles(files), entry });
  saveLocalList([...list, sketch]);
  return sketch;
}

export function renameLocalSketch(id, title) {
  saveLocalList(localSketches().map((s) => (s.id === id ? { ...s, title } : s)));
}

export function deleteLocalSketch(id) {
  saveLocalList(localSketches().filter((s) => s.id !== id));
  storage.remove(`sketch:${id}`);
}

export function forgetNewDraft() {
  storage.remove("draft:new");
}
