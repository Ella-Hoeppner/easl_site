// Running programs: each run gets a fresh iframe (runner.html), so stopping
// or rerunning a program frees everything it held. A program is compiled
// once for each version of its files (with every one of its `@cpu` entry
// points), and later runs of that version, at any of its entry points,
// start from the compiled program without compiling it again. Also the console under
// the canvas, the bar above it listing the document's `@cpu` entry points,
// and taking a picture of the canvas (a sketch's thumbnail).

import { $, el, storage } from "./util.js?v=__VERSION__";

const MAX_CONSOLE_LINES = 1000;

// How many compiled programs are kept, most recently used first.
const MAX_COMPILED_PROGRAMS = 8;

/** A key naming a version of a program: its main file and the text of
 * every file (two 32-bit FNV-1a hashes, with the length). */
function programKey(files, mainPath) {
  let a = 0x811c9dc5;
  let b = 0x01000193;
  let length = 0;
  for (const text of [mainPath, ...files.flatMap(({ path, text }) => [path, text])]) {
    for (let i = 0; i < text.length; i++) {
      const c = text.charCodeAt(i);
      a = Math.imul(a ^ c, 0x01000193);
      b = Math.imul(b ^ c, 0x5bd1e995);
    }
    a = Math.imul(a ^ 0xff, 0x01000193);
    length += text.length + 1;
  }
  return `${(a >>> 0).toString(16)}${(b >>> 0).toString(16)}-${length}`;
}

export const supportsWebGPU = Boolean(navigator.gpu);

export class Runner {
  /**
   * `reveal(path, line)` shows a place in the document (for links in the
   * console), and `onState(state)` hears each change of the run's state:
   * "stopped", "checking", "starting", "running", "paused", or "finished".
   */
  constructor({ reveal, onState }) {
    this.reveal = reveal;
    this.onState = onState;
    this.current = null;
    this.token = 0;
    this.state = "stopped";
    // Compiled programs (`{bytes, entryPoints}`) by `programKey`.
    this.compiled = new Map();
    // The level programs' audio plays at (0 to 1), and whether it's muted,
    // remembered across visits.
    const saved = Number(storage.get("volume"));
    this.volume = storage.get("volume") !== null && saved >= 0 && saved <= 1 ? saved : 1;
    this.muted = storage.get("muted") === "true";
  }

  /** The level the running program's audio plays at. */
  get outputVolume() {
    return this.muted ? 0 : this.volume;
  }

  setVolume(volume) {
    this.volume = volume;
    storage.set("volume", String(volume));
    this.applyVolume();
  }

  setMuted(muted) {
    this.muted = muted;
    storage.set("muted", String(muted));
    this.applyVolume();
  }

  applyVolume() {
    this.current?.iframe.contentWindow.setEaslVolume?.(this.outputVolume);
  }

  /** The compiled program for `key`, if there is one (marking it the most
   * recently used). */
  compiledProgram(key) {
    const compiled = this.compiled.get(key);
    if (compiled) {
      this.compiled.delete(key);
      this.compiled.set(key, compiled);
    }
    return compiled ?? null;
  }

  keepCompiled(key, compiled) {
    this.compiled.set(key, compiled);
    while (this.compiled.size > MAX_COMPILED_PROGRAMS) {
      this.compiled.delete(this.compiled.keys().next().value);
    }
  }

  setState(state) {
    this.state = state;
    document.body.dataset.run = state;
    const pause = $("pause");
    const resuming = state === "paused";
    pause.title = resuming ? "Resume" : "Pause";
    pause.setAttribute("aria-label", pause.title);
    pause.classList.toggle("resume", resuming);
    pause.disabled = state !== "running" && state !== "paused";
    $("stop").disabled = state !== "running" && state !== "paused";
    this.onState?.(state);
  }

  /**
   * Adds a line to the console. A place in the document (`at`, as
   * `{path, line}` with a one-based line) makes the line a link to it.
   */
  log(kind, text, at = null) {
    const output = $("console");
    const entry = el("div", { className: `line ${kind}` });
    if (at) {
      entry.append(
        el("button", {
          className: "line-link",
          onclick: () => this.reveal(at.path, at.line),
        }, at.label ?? `line ${at.line}`),
        ": ",
      );
    }
    entry.append(text);
    output.append(entry);
    while (output.childElementCount > MAX_CONSOLE_LINES) output.firstElementChild.remove();
    output.scrollTop = output.scrollHeight;
    $("console-empty").hidden = true;
  }

  clearConsole() {
    $("console").replaceChildren();
    $("console-empty").hidden = false;
  }

  /** Stops the program (if one is running) and forgets it. */
  stop() {
    this.token++;
    this.current?.iframe.remove();
    this.current = null;
    this.setState("stopped");
  }

  togglePause() {
    if (!this.current) return;
    const runner = this.current.iframe.contentWindow;
    if (this.current.program.paused) {
      this.current.program.resume();
      runner.setEaslPaused?.(false);
      this.setState("running");
    } else {
      this.current.program.pause();
      runner.setEaslPaused?.(true);
      this.setState("paused");
    }
  }

  /**
   * Checks, then runs, a program: `check()` resolves to the check of its
   * main file (`{diagnostics, failure}`), and the program runs only when
   * that's clean (the compiler, which runs on the page's thread, can hang
   * or crash on programs the check rejects). `files` are the document's
   * files, `entry` the `@cpu` entry point to run as `{file, name}`; the
   * program is compiled with the entry's file as its main file, unless
   * that version of it has been compiled already.
   */
  async run({ files, entry, check }) {
    if (!supportsWebGPU) return;
    const token = ++this.token;
    this.setState("checking");
    const result = await check();
    if (token !== this.token) return;
    if (result.failure || result.diagnostics.length > 0) {
      this.stop();
      this.clearConsole();
      if (result.failure) {
        this.log("error", result.failure);
      } else {
        for (const diagnostic of result.diagnostics) {
          const line = diagnostic.range[0] + 1;
          this.log("error", diagnostic.message, {
            path: entry.file,
            line,
            label: files.length > 1 ? `${entry.file}:${line}` : `line ${line}`,
          });
        }
        const first = result.diagnostics[0];
        this.reveal(entry.file, first.range[0] + 1);
      }
      return;
    }
    this.current?.iframe.remove();
    this.current = null;
    this.clearConsole();
    this.setState("starting");
    const iframe = el("iframe", {
      title: "The program's output",
      allow: "midi; autoplay",
      src: new URL("runner.html?v=__VERSION__", import.meta.url).href,
    });
    $("stage").append(iframe);
    // A run another one replaced while it started takes its iframe with it.
    const abandoned = () => {
      if (token === this.token) return false;
      iframe.remove();
      return true;
    };
    await new Promise((resolve) => iframe.addEventListener("load", resolve, { once: true }));
    if (abandoned()) return;
    let program;
    try {
      const key = programKey(files, entry.file);
      let compiled = this.compiledProgram(key);
      if (!compiled) {
        const result = await iframe.contentWindow.compileEasl({
          files: files.map(({ path, text }) => ({ path, source: text })),
          mainPath: entry.file,
        });
        // Copied out of the iframe, which goes away with its run.
        compiled = {
          bytes: new Uint8Array(result.bytes),
          entryPoints: Array.from(result.entryPoints),
        };
        this.keepCompiled(key, compiled);
      }
      if (abandoned()) return;
      program = await iframe.contentWindow.runEasl({
        bytes: compiled.bytes,
        entry: entry.name,
        volume: this.outputVolume,
        onPrint: (line) => {
          if (token === this.token) this.log("print", line);
        },
        onAudioError: (message) => {
          if (token === this.token) this.log("error", message);
        },
      });
    } catch (error) {
      if (abandoned()) return;
      this.log("error", String(error?.message ?? error).replace(/^easl: /, ""));
      this.setState("stopped");
      return;
    }
    if (abandoned()) return;
    this.current = { iframe, program };
    this.setState("running");
    program.finished.then(
      () => {
        if (token !== this.token) return;
        this.log("info", "The program finished.");
        this.setState("finished");
      },
      (error) => {
        if (token !== this.token) return;
        this.log("error", String(error?.message ?? error).replace(/^easl: /, ""));
        this.setState("stopped");
      },
    );
  }

  /** A picture of the program's canvas as it's showing now (an image Blob
   * at most `width` wide), or null when nothing is running. */
  async capture(width = 480) {
    if (!this.current || this.state !== "running") return null;
    try {
      const picture = await this.current.iframe.contentWindow.captureFrame(width);
      // A Blob from the iframe's realm isn't an `instanceof Blob` here.
      return picture ? new Blob([picture], { type: picture.type }) : null;
    } catch {
      return null;
    }
  }
}

// More entry points than this are offered in a dropdown.
const MAX_ENTRY_BUTTONS = 3;

const entryLabel = (entry, multipleFiles) =>
  multipleFiles ? `${entry.file} · ${entry.name}` : entry.name;

/**
 * The right side of the bar above the canvas: the document's `@cpu` entry
 * points, the one that runs highlighted (as buttons, or a dropdown when
 * there are more than a few). Choosing one calls `choose(entry)`.
 */
export function renderEntries(host, { entries, current, multipleFiles, choose }) {
  if (entries.length === 0) {
    host.replaceChildren(
      el("span", { className: "entries-empty" }, "No ", el("code", {}, "@cpu"), " entry point"),
    );
    return;
  }
  if (entries.length > MAX_ENTRY_BUTTONS) {
    const index = entries.findIndex((e) => e.file === current?.file && e.name === current?.name);
    const select = el("select", {
      className: "entry-select",
      title: "The entry point to run",
      onchange: () => choose(entries[select.selectedIndex]),
    }, ...entries.map((entry, i) =>
      el("option", { value: String(i), selected: i === index }, entryLabel(entry, multipleFiles))));
    host.replaceChildren(el("label", { className: "entry-select-label" }, "Entry ", select));
    return;
  }
  host.replaceChildren(
    ...entries.map((entry) => {
      const selected = entry.file === current?.file && entry.name === current?.name;
      return el(
        "button",
        {
          className: `entry${selected ? " current" : ""}`,
          title: `Run ${entry.name} (in ${entry.file})`,
          attrs: { "aria-pressed": String(selected) },
          onclick: () => choose(entry),
        },
        multipleFiles ? el("span", { className: "entry-file" }, `${entry.file} · `) : null,
        entry.name,
      );
    }),
  );
}
