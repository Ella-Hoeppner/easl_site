// The page's layout: the draggable splitters between the editor and the
// output, and between the canvas and the panels under it, and the
// keyboard-shortcut help.

import { $, IS_MAC, el, storage } from "./util.js?v=__VERSION__";

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

export function setUpSplitters() {
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

  // Between the canvas and the panels: the panels' height, as a fraction
  // of the output pane, from collapsed to their tabs up to most of the
  // pane.
  const output = document.querySelector(".output-pane");
  const setPanelFraction = fraction(output, "--console-fraction", "console-split", 0, 0.85);
  draggable($("console-splitter"), (event) => {
    const box = output.getBoundingClientRect();
    const bar = document.querySelector(".panel-bar").getBoundingClientRect();
    const splitterHeight = $("console-splitter").getBoundingClientRect().height;
    setPanelFraction((box.bottom - event.clientY - splitterHeight / 2 - bar.height) / box.height);
  });
}

const key = (mac, other) => (IS_MAC ? mac : other);

const SHORTCUTS = [
  ["Run", key("⌘ Enter", "Ctrl+Enter")],
  ["Save: format (if Format on save is on), save, and run", key("⌘ S", "Ctrl+S")],
  ["Format", key("⇧ ⌥ F", "Shift+Alt+F")],
  ["Expand selection to the enclosing form", key("⌃ W or ⌃ ⇧ ⌘ →", "Shift+Alt+→")],
  ["Shrink selection", key("⌃ ⇧ ⌘ ←", "Shift+Alt+←")],
  ["Cursor to start / end of form", key("⌃ A / ⌃ D, or ⌃ ⌥ ↑ / ↓", "Ctrl+Alt+↑ / ↓")],
  ["Move form backward / forward", key("⌃ ⇧ A / ⌃ ⇧ D, or ⌃ ⌥ ← / →", "Ctrl+Alt+← / →")],
  ["Go to definition (in any file)", key("F12 or ⌘ click", "F12 or Ctrl+click")],
  ["Next problem", key("F8", "F8")],
  ["Suggestions", key("⌃ Space", "Ctrl+Space")],
  ["Toggle comment", key("⌘ /", "Ctrl+/")],
  ["All commands", "F1"],
];

export function setUpHelp() {
  $("shortcuts").replaceChildren(
    ...SHORTCUTS.map(([action, keys]) =>
      el("tr", {}, el("td", {}, action), el("td", {}, el("kbd", {}, keys)))),
  );
  $("help").addEventListener("click", () => $("help-dialog").showModal());
  $("help-close").addEventListener("click", () => $("help-dialog").close());
}
