// The editor's language features, from the analysis worker: checking the
// file being edited (with the sketch's other files, which it can import),
// and Monaco's hover, go-to-definition, signature help, completion,
// formatting, selection ranges, and semantic tokens.

import { callHeadBefore } from "./easl-language.js?v=__VERSION__";
import { ANALYSIS_TIMEOUT, quietly } from "./analysis.js?v=__VERSION__";
import { relativePath } from "./util.js?v=__VERSION__";

// How long typing must pause before the file is checked.
const ANALYSIS_DELAY = 250;

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

/**
 * Checks the files of the open document as they're edited.
 *
 * `context` gives it what it works on: `monaco`, `analysis` (the worker
 * client), `doc()` (the open document), `revision()` (a counter that
 * changes whenever any of its files does), `modelFor(doc, path)`, and
 * `onChecked(result)`, called with each finished check of the file in
 * the editor.
 */
export class Checker {
  constructor(context) {
    this.context = context;
    // The last check: which model and revision it describes, and how it
    // went. Hover and go-to-definition answer only for that text.
    this.checked = { model: null, revision: -1, diagnostics: [], failure: null };
    this.running = null;
    this.timer = null;
  }

  schedule(model, delay = ANALYSIS_DELAY) {
    clearTimeout(this.timer);
    this.timer = setTimeout(() => this.check(model), delay);
  }

  /** Whether the last check describes `model` as everything is now. */
  isCurrent(model) {
    return this.checked.model === model && this.checked.revision === this.context.revision();
  }

  /** Checks `model`'s file as the main file of its own program, unless
   * that's already been done for the current text of every file. */
  check(model) {
    clearTimeout(this.timer);
    const revision = this.context.revision();
    if (this.isCurrent(model)) return Promise.resolve(this.checked);
    if (this.running?.model === model && this.running.revision === revision) {
      return this.running.promise;
    }
    const doc = this.context.doc();
    const files = Object.fromEntries(doc.files.map((file) => [file.path, file.text]));
    const promise = this.context.analysis
      .request("analyze", [model.path, files], ANALYSIS_TIMEOUT)
      .then(
        (diagnostics) => ({ diagnostics, failure: null }),
        (error) => ({ diagnostics: [], failure: failureMessage(error, files) }),
      )
      .then(({ diagnostics, failure }) => {
        const result = { model, revision, diagnostics, failure, doc };
        if (this.context.revision() === revision) {
          this.checked = result;
          this.showMarkers(result);
          this.context.onChecked(result);
        }
        return result;
      });
    this.running = { model, revision, promise };
    return promise;
  }

  showMarkers({ model, diagnostics, failure, doc }) {
    const { monaco, modelFor } = this.context;
    const toRange = rangeMaker(monaco);
    const markers = diagnostics.map((diagnostic) => {
      const range = toRange(diagnostic.range);
      return {
        severity: monaco.MarkerSeverity.Error,
        message: diagnostic.message,
        ...range,
        // A zero-width range (an error with no exact place) still shows.
        endColumn: range.isEmpty() ? range.endColumn + 1 : range.endColumn,
        relatedInformation: diagnostic.related.map((related) => ({
          resource: modelFor(doc, related.path)?.uri ?? model.uri,
          message: related.path === model.path ? "related location" : `in ${related.path}`,
          ...toRange(related.range),
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
  }
}

/** [line, character, line, character] (zero-based, as the worker speaks)
 * to a Monaco range. */
export const rangeMaker =
  (monaco) =>
  ([sl, sc, el, ec]) =>
    new monaco.Range(sl + 1, sc + 1, el + 1, ec + 1);

/**
 * What to say when the compiler couldn't check the files: it took too long,
 * or it crashed. Both currently happen on recursive functions (which easl
 * doesn't allow, but the compiler doesn't yet reject cleanly), so those are
 * named when there are any.
 */
function failureMessage(error, files) {
  const recursive = Object.values(files).flatMap(recursiveFunctions);
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

/**
 * Registers the language features with Monaco. `context` is the checker's
 * (see `Checker`) plus `checker`, `entriesAt(model, position)` (the `@cpu`
 * entry whose annotation is at a position, if any), and `runEntryCommand`
 * (the id of the command that runs an entry, for hover links).
 */
export async function registerFeatures(context) {
  const { monaco, analysis, checker, modelFor } = context;
  const toRange = rangeMaker(monaco);

  monaco.languages.registerHoverProvider("easl", {
    async provideHover(model, position) {
      const entry = context.entryAt(model, position);
      if (entry) {
        const args = encodeURIComponent(JSON.stringify([entry.file, entry.name]));
        return {
          range: toRange(entry.range),
          contents: [
            {
              value: `[▶ Run \`${entry.name}\`](command:${context.runEntryCommand}?${args})`,
              isTrusted: true,
            },
            { value: "This `@cpu` function is an entry point: running it starts the program." },
          ],
        };
      }
      if (!checker.isCurrent(model)) return null;
      const hover = await quietly(
        analysis.request("hover", [position.lineNumber - 1, position.column - 1]),
      );
      if (!hover || !checker.isCurrent(model)) return null;
      return {
        range: toRange(hover.range),
        contents: hover.contents.map((value) => ({ value })),
      };
    },
  });

  monaco.languages.registerDefinitionProvider("easl", {
    async provideDefinition(model, position) {
      if (!checker.isCurrent(model)) return null;
      const locations = await quietly(
        analysis.request("definition", [position.lineNumber - 1, position.column - 1]),
      );
      if (!locations || !checker.isCurrent(model)) return null;
      const doc = context.doc();
      return locations.flatMap(({ path, range }) => {
        const target = modelFor(doc, path);
        return target ? [{ uri: target.uri, range: toRange(range) }] : [];
      });
    },
  });

  // Inside a call, the called function's declarations (each overload's).
  monaco.languages.registerSignatureHelpProvider("easl", {
    signatureHelpTriggerCharacters: [" ", "("],
    signatureHelpRetriggerCharacters: [" "],
    async provideSignatureHelp(model, position) {
      if (!checker.isCurrent(model)) return null;
      const before = model.getValueInRange(
        new monaco.Range(1, 1, position.lineNumber, position.column),
      );
      const head = callHeadBefore(before);
      if (head === null) return null;
      const headPosition = model.getPositionAt(head);
      const hover = await quietly(
        analysis.request("hover", [headPosition.lineNumber - 1, headPosition.column - 1]),
      );
      const declarations = (hover?.contents ?? [])
        .map((content) => content.replace(/^```easl\n|\n```$/g, ""))
        .filter((declaration) => declaration.startsWith("("));
      if (declarations.length === 0 || !checker.isCurrent(model)) return null;
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

  monaco.languages.registerCompletionItemProvider("easl", {
    triggerCharacters: ["-", "/", '"'],
    async provideCompletionItems(model, position) {
      const before = model.getValueInRange(
        new monaco.Range(position.lineNumber, 1, position.lineNumber, position.column),
      );
      // Inside an import's path: the sketch's other files.
      const importPath = before.match(/\(import\s+(?:[^\s"()]+\s+)?"([^"]*)$/);
      if (importPath) {
        const typed = importPath[1];
        const range = new monaco.Range(
          position.lineNumber,
          position.column - typed.length,
          position.lineNumber,
          position.column,
        );
        return {
          suggestions: context
            .doc()
            .files.filter((file) => file.path !== model.path)
            .map((file) => ({
              label: relativePath(model.path, file.path),
              kind: monaco.languages.CompletionItemKind.File,
              insertText: relativePath(model.path, file.path),
              range,
            })),
        };
      }
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
          kind: monaco.languages.CompletionItemKind[COMPLETION_KINDS[item.kind] ?? "Text"],
          sortText: item.sortText,
          insertText: item.label,
          range: toRange(item.range),
        })),
      };
    },
  });

  monaco.languages.registerDocumentFormattingEditProvider("easl", {
    async provideDocumentFormattingEdits(model) {
      const formatted = await quietly(analysis.request("format", [model.getValue()]));
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

  // The semantic tokens color what the tokenizer can't, like a form
  // commented out with `#_`. Their bracket types (by nesting depth) are
  // left to the editor's own bracket colors.
  const tokenTypes = await quietly(analysis.request("semanticTokenTypes"));
  if (tokenTypes) {
    monaco.languages.registerDocumentSemanticTokensProvider("easl", {
      getLegend: () => ({ tokenTypes, tokenModifiers: [] }),
      async provideDocumentSemanticTokens(model) {
        const data = await quietly(analysis.request("semanticTokens", [model.getValue()]));
        return data ? { data: new Uint32Array(data) } : null;
      },
      releaseDocumentSemanticTokens() {},
    });
  }
}
