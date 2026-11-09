// The client side of the analysis worker (analysis-worker.js), which runs
// easl_lsp's language support compiled to WebAssembly. A request that takes
// too long, or a compiler panic, restarts the worker, so the editor never
// freezes.

// How long an analysis may take before the worker is presumed stuck.
export const ANALYSIS_TIMEOUT = 10_000;
// How long other requests may take.
const REQUEST_TIMEOUT = 5_000;

export class AnalysisError extends Error {}

export class Analysis {
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

/** `promise`, with a failure turned into null (for optional features). */
export const quietly = (promise) => promise.catch(() => null);
