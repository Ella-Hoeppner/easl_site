// Running programs: each run gets a fresh iframe (runner.html), so stopping
// or rerunning a program frees everything it held. Also the console under
// the canvas, the bar above it listing the document's `@cpu` entry points,
// and taking a picture of the canvas (a sketch's thumbnail).

import { $, el } from "./util.js?v=__VERSION__";

const MAX_CONSOLE_LINES = 1000;

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
  }

  setState(state) {
    this.state = state;
    document.body.dataset.run = state;
    $("pause").textContent = state === "paused" ? "Resume" : "Pause";
    $("pause").disabled = state !== "running" && state !== "paused";
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
    if (this.current.program.paused) {
      this.current.program.resume();
      this.setState("running");
    } else {
      this.current.program.pause();
      this.setState("paused");
    }
  }

  /**
   * Checks, then runs, a program: `check()` resolves to the check of its
   * main file (`{diagnostics, failure}`), and the program runs only when
   * that's clean (the compiler, which runs on the page's thread, can hang
   * or crash on programs the check rejects). `files` are the document's
   * files, `entry` the `@cpu` entry point to run as `{file, name}`.
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
    await new Promise((resolve) => iframe.addEventListener("load", resolve, { once: true }));
    if (token !== this.token) return;
    let program;
    try {
      program = await iframe.contentWindow.runEasl({
        files: files.map(({ path, text }) => ({ path, source: text })),
        mainPath: entry.file,
        entry: entry.name,
        onPrint: (line) => {
          if (token === this.token) this.log("print", line);
        },
        onAudioError: (message) => {
          if (token === this.token) this.log("error", message);
        },
      });
    } catch (error) {
      if (token !== this.token) return;
      this.log("error", String(error?.message ?? error));
      this.setState("stopped");
      return;
    }
    if (token !== this.token) return;
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

/**
 * The bar above the canvas: the document's `@cpu` entry points, the one
 * that runs highlighted. Choosing one calls `choose(entry)`.
 */
export function renderEntries(host, { entries, current, multipleFiles, choose }) {
  if (entries.length === 0) {
    host.replaceChildren(
      el("span", { className: "entries-empty" }, "No ", el("code", {}, "@cpu"), " entry point"),
    );
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
        el("span", { className: "entry-play", attrs: { "aria-hidden": "true" } }, "▶"),
        multipleFiles ? el("span", { className: "entry-file" }, `${entry.file} · `) : null,
        entry.name,
      );
    }),
  );
}
