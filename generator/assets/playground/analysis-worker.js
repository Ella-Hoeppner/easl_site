// The playground's language support, off the page's main thread: easl's
// analysis (playground/analysis), compiled to WebAssembly. The page
// sends `{ id, method, args }` and gets back `{ id, result }` or
// `{ id, error, dead }`. A compiler panic ends the WebAssembly instance
// (`dead`), and its message arrives first as `{ panic }`; the page then
// starts a new worker.

import init, * as easl from "./analysis/easl_playground_analysis.js?v=__VERSION__";

self.easlAnalysisPanic = (message) => self.postMessage({ panic: message });

// The version query (see playground.rs) keeps the browser from reusing a
// module cached from an earlier build.
const ready = init({
  module_or_path: new URL(
    "analysis/easl_playground_analysis_bg.wasm?v=__VERSION__",
    import.meta.url,
  ),
});

const parse = (json) => JSON.parse(json);

const methods = {
  analyze: (text) => parse(easl.analyze(text)),
  hover: (line, character) => parse(easl.hover(line, character)),
  definition: (line, character) => parse(easl.definition(line, character)),
  completions: (text, line, character) =>
    parse(easl.completions(text, line, character)),
  format: (text) => easl.format(text) ?? null,
  semanticTokenTypes: () => parse(easl.semanticTokenTypes()),
  semanticTokens: (text) => withoutOverlaps(easl.semanticTokens(text)),
  structural: (command, text, ...selection) =>
    parse(easl.structural(command, text, ...selection)),
  // Each enclosing form's range, innermost first, for the editor's
  // expand-selection.
  selectionRanges: (text, line, character) => {
    const ranges = [];
    let range = [line, character, line, character];
    for (let i = 0; i < 256; i++) {
      const next = parse(easl.structural("expandSelection", text, ...range));
      if (!next || next.every((n, j) => n === range[j])) break;
      ranges.push(next);
      range = next;
    }
    return ranges;
  },
};

self.onmessage = async ({ data: { id, method, args } }) => {
  await ready;
  try {
    self.postMessage({ id, result: methods[method](...args) });
  } catch (error) {
    self.postMessage({
      id,
      error: String(error),
      dead: error instanceof WebAssembly.RuntimeError,
    });
  }
};

/**
 * `data` (semantic tokens in the LSP's relative encoding, five numbers per
 * token) without tokens that start inside the previous one, which the
 * editor would warn about.
 */
function withoutOverlaps(data) {
  const kept = [];
  let line = 0;
  let start = 0;
  let keptLine = 0;
  let keptStart = 0;
  let keptEnd = 0;
  for (let i = 0; i < data.length; i += 5) {
    line += data[i];
    start = data[i] === 0 ? start + data[i + 1] : data[i + 1];
    if (kept.length > 0 && line === keptLine && start < keptEnd) continue;
    kept.push(
      line - keptLine,
      line === keptLine ? start - keptStart : start,
      data[i + 2],
      data[i + 3],
      data[i + 4],
    );
    keptLine = line;
    keptStart = start;
    keptEnd = start + data[i + 2];
  }
  return kept;
}
