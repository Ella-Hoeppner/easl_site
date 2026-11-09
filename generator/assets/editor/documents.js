// What the editor edits: a sketch, made of one or more easl files (in
// folders, if it likes), of one of two kinds:
//
// - "sketch": a sketch on the server. Its owner's edits are kept as a
//   draft in this browser until they're saved; anyone else can edit it
//   here too (kept as a draft) and fork it to save a copy.
// - "new": a sketch that hasn't been saved yet, kept as a draft. One opened
//   from a link carrying its program (`/new#code=…`, as the docs link to
//   programs) has a draft of its own, so it doesn't replace the visitor's.
//
// Each document also remembers which `@cpu` entry point it runs (`entry`,
// `{file, name}`), since a sketch can have several, and any folders that
// don't hold a file yet (which only drafts keep).

import { storage } from "./util.js?v=__VERSION__";

export const MAIN_FILE = "main.easl";

const sameFiles = (a, b) =>
  a.length === b.length &&
  a.every((file, i) => file.path === b[i].path && file.text === b[i].text);

const sameEntry = (a, b) =>
  (a?.file ?? null) === (b?.file ?? null) && (a?.name ?? null) === (b?.name ?? null);

const copyFiles = (files) => files.map(({ path, text }) => ({ path, text }));

/** A stored `{files, entry, folders}`, or null when it isn't one. */
function readStored(stored) {
  if (stored && Array.isArray(stored.files) && stored.files.length > 0) {
    return {
      files: copyFiles(stored.files),
      entry: stored.entry ?? null,
      folders: Array.isArray(stored.folders) ? [...stored.folders] : [],
    };
  }
  return null;
}

export class Doc {
  constructor({ kind, id = null, title, files, entry = null, folders = [], baseline, sketch = null, code = null }) {
    this.kind = kind;
    this.id = id;
    this.title = title;
    this.files = copyFiles(files);
    this.entry = entry;
    // Folders that hold no file (a folder with files is implied by them).
    this.folders = [...folders];
    // What the document is when it has no unsaved changes.
    this.baseline = { files: copyFiles(baseline.files), entry: baseline.entry ?? null };
    // The server's description of a "sketch" document.
    this.sketch = sketch;
    // The encoded program a "new" document was opened from, if any.
    this.code = code;
  }

  /** A key naming the document in this browser. */
  get key() {
    if (this.kind === "sketch") return `sketch:${this.id}`;
    return this.id ? `new:${this.id}` : "new";
  }

  /** The address that opens the document. */
  get url() {
    if (this.kind === "sketch") return `/s/${this.id}`;
    return this.code ? `/new#code=${this.code}` : "/new";
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
    return (
      !sameFiles(this.files, this.baseline.files) ||
      !sameEntry(this.entry, this.baseline.entry)
    );
  }

  /** Keeps the document's current state in this browser: a draft while it
   * has unsaved changes (or empty folders), none otherwise. */
  persist() {
    const draftKey = `draft:${this.key}`;
    if (!this.isDirty() && this.folders.length === 0) {
      storage.remove(draftKey);
      return;
    }
    const state = { files: this.files, entry: this.entry, folders: this.folders };
    if (this.kind === "sketch") state.base = this.sketch.updated_at;
    storage.setJSON(draftKey, state);
  }

  /** Returns to the state with no unsaved changes, forgetting the draft. */
  revert() {
    this.files = copyFiles(this.baseline.files);
    this.entry = this.baseline.entry;
    this.folders = [];
    storage.remove(`draft:${this.key}`);
  }

  /** Marks the current state as saved (after the server accepted it). */
  markSaved(sketch) {
    this.sketch = sketch;
    this.title = sketch.title;
    this.baseline = { files: copyFiles(this.files), entry: this.entry };
    this.persist();
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

/** A server sketch, with this browser's draft of it if there's one. The
 * draft is used only if it was made from the sketch as it is now; one made
 * from an older version comes back separately as `staleDraft`. */
export function sketchDoc(sketch) {
  const baseline = {
    files: sketch.files.map(({ path, content }) => ({ path, text: content })),
    entry: sketch.entry_file ? { file: sketch.entry_file, name: sketch.entry_name } : null,
  };
  const stored = storage.getJSON(`draft:sketch:${sketch.id}`);
  const draft = readStored(stored);
  const current = draft && stored.base === sketch.updated_at;
  const doc = new Doc({
    kind: "sketch",
    id: sketch.id,
    title: sketch.title,
    ...(current ? draft : baseline),
    baseline,
    sketch,
  });
  return { doc, staleDraft: draft && !current ? draft : null };
}

/** A new sketch: from `starter`, or from `code` (the program a link
 * carries, as `{source, encoded, id}`), with its draft if there's one. */
export function newDoc(starter, code = null) {
  const baseline = { files: [{ path: MAIN_FILE, text: code?.source ?? starter }], entry: null };
  const doc = new Doc({
    kind: "new",
    id: code?.id ?? null,
    title: "Untitled sketch",
    files: baseline.files,
    baseline,
    code: code?.encoded ?? null,
  });
  const draft = readStored(storage.getJSON(`draft:${doc.key}`));
  if (draft) Object.assign(doc, draft);
  return doc;
}

export function forgetDraft(doc) {
  storage.remove(`draft:${doc.key}`);
}
