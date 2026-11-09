// Small helpers shared by the editor's modules: DOM building, the
// browser's storage, dialogs, and formatting.

export const $ = (id) => document.getElementById(id);

export const IS_MAC = /Mac|iPhone|iPad/.test(navigator.platform);

/**
 * A new element: `el("a", { href, className: "x", onclick }, "text", child)`.
 * Properties go on the element (`dataset`, `style` and `attrs` are merged
 * in), and children are appended, strings as text.
 */
export function el(tag, props = {}, ...children) {
  const element = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (value === undefined || value === null) continue;
    if (key === "dataset" || key === "style") {
      Object.assign(element[key], value);
    } else if (key === "attrs") {
      for (const [name, attr] of Object.entries(value)) {
        element.setAttribute(name, attr);
      }
    } else {
      element[key] = value;
    }
  }
  element.append(...children.flat().filter((child) => child != null));
  return element;
}

// ---- storage ----

const STORAGE_PREFIX = "easl-playground:";

/** The browser's localStorage, which may be unavailable (private windows,
 * blocked site data): reads then find nothing and writes do nothing. */
export const storage = {
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
    } catch {}
  },
  remove(key) {
    try {
      localStorage.removeItem(STORAGE_PREFIX + key);
    } catch {}
  },
  getJSON(key) {
    try {
      return JSON.parse(this.get(key) ?? "null");
    } catch {
      return null;
    }
  },
  setJSON(key, value) {
    this.set(key, JSON.stringify(value));
  },
};

// ---- dialogs ----

/**
 * Asks for a line of text in the page's prompt dialog, resolving to the
 * trimmed answer, or null when it's canceled. `validate` returns an error
 * message for an answer that can't be used, shown under the field.
 */
export function promptText({ title, label, value = "", confirm = "OK", validate }) {
  const dialog = $("prompt-dialog");
  const input = $("prompt-input");
  const error = $("prompt-error");
  $("prompt-title").textContent = title;
  $("prompt-label").textContent = label;
  $("prompt-confirm").textContent = confirm;
  input.value = value;
  error.textContent = "";
  dialog.showModal();
  input.select();
  return new Promise((resolve) => {
    const submit = (event) => {
      event.preventDefault();
      const answer = input.value.trim();
      const problem = validate?.(answer);
      if (problem) {
        error.textContent = problem;
        input.focus();
        return;
      }
      finish(answer);
    };
    const cancel = () => finish(null);
    const finish = (answer) => {
      $("prompt-form").removeEventListener("submit", submit);
      $("prompt-cancel").removeEventListener("click", cancel);
      dialog.removeEventListener("close", cancel);
      if (dialog.open) dialog.close();
      resolve(answer);
    };
    $("prompt-form").addEventListener("submit", submit);
    $("prompt-cancel").addEventListener("click", cancel);
    dialog.addEventListener("close", cancel);
  });
}

/** Shows `message` briefly at the bottom of the page. */
export function toast(message, kind = "info") {
  const host = $("toasts");
  const note = el("div", { className: `toast ${kind}`, role: "status" }, message);
  host.append(note);
  setTimeout(() => note.classList.add("leaving"), 2600);
  setTimeout(() => note.remove(), 3000);
}

// ---- formatting ----

const TIME_UNITS = [
  ["year", 365 * 24 * 3600],
  ["month", 30 * 24 * 3600],
  ["week", 7 * 24 * 3600],
  ["day", 24 * 3600],
  ["hour", 3600],
  ["minute", 60],
];

/** A unix time (seconds) as "3 days ago". */
export function timeAgo(seconds) {
  const elapsed = Date.now() / 1000 - seconds;
  const format = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  for (const [unit, length] of TIME_UNITS) {
    if (elapsed >= length) return format.format(-Math.floor(elapsed / length), unit);
  }
  return "just now";
}

/** A unix time (seconds) as a full date, for tooltips. */
export const fullDate = (seconds) => new Date(seconds * 1000).toLocaleString();

/**
 * `text` as nodes: plain text with line breaks kept, and http(s) URLs as
 * links. Nothing in it is parsed as HTML.
 */
export function richText(text) {
  const nodes = [];
  const url = /https?:\/\/[^\s<>"']+[^\s<>"'.,;:!?)\]]/g;
  let last = 0;
  for (const match of text.matchAll(url)) {
    nodes.push(text.slice(last, match.index));
    nodes.push(
      el("a", { href: match[0], rel: "nofollow ugc noopener", target: "_blank" }, match[0]),
    );
    last = match.index + match[0].length;
  }
  nodes.push(text.slice(last));
  return nodes;
}

// ---- sketch file paths ----

/**
 * Why `path` can't name a sketch's file, or null when it can: relative,
 * `/`-separated names of letters, digits, `.`, `_` and `-`, ending in
 * `.easl` (the server checks the same).
 */
export function pathProblem(path) {
  if (!path) return "A file needs a name.";
  if (path.length > 128) return "That name is too long.";
  if (!path.endsWith(".easl")) return "Easl files end in .easl.";
  const segments = path.split("/");
  if (segments.some((segment) => segment === "" || segment === "." || segment === "..")) {
    return "Use names separated by single slashes, without . or .. folders.";
  }
  if (!segments.every((segment) => /^[A-Za-z0-9._-]+$/.test(segment))) {
    return "Use only letters, digits, and . _ - in file names.";
  }
  return null;
}

/** The path of `target` relative to the directory holding `from`. */
export function relativePath(from, target) {
  const fromDir = from.split("/").slice(0, -1);
  const parts = target.split("/");
  let common = 0;
  while (common < fromDir.length && common < parts.length - 1 && fromDir[common] === parts[common]) {
    common++;
  }
  return [...fromDir.slice(common).map(() => ".."), ...parts.slice(common)].join("/");
}
