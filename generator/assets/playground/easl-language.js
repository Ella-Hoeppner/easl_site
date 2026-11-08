// easl for the Monaco editor: tokenizing (colors close to the docs' code
// blocks), bracket and quote behavior, and light and dark themes. The
// language support that needs the compiler (diagnostics, hover, and so on)
// is in playground.js, backed by the analysis worker.

const KEYWORDS = [
  "defn", "def", "var", "override", "struct", "enum", "import", "mod", "use",
  "let", "fn", "if", "when", "match", "do", "for", "while", "break",
  "continue", "return", "discard", "->", "<>",
];

const NUMBER = "~^-?(\\d+\\.?\\d*|\\.\\d+)[fiu]?$";
const BOOLEAN = "~^(true|false)$";
const TYPE =
  "~^(f32|i32|u32|bool|vec[234][fiub]?|mat[234]x[234][fiu]?|[A-Z][\\w-]*)$";

// A name, a number, or an operator symbol like `+` or `->`.
const SYMBOL = /[^\s()\[\]{}";:~@#]+/;

const tokenizer = {
  defaultToken: "identifier",
  keywords: KEYWORDS,
  brackets: [
    { open: "(", close: ")", token: "delimiter.parenthesis" },
    { open: "[", close: "]", token: "delimiter.square" },
    { open: "{", close: "}", token: "delimiter.curly" },
  ],
  tokenizer: {
    root: [
      { include: "@whitespace" },
      // `#_` comments out the next form; the semantic tokens from the
      // analysis worker color the form itself.
      [/#_/, "comment"],
      [/"/, { token: "string.quote", next: "@string" }],
      [/@[^\s()\[\]{}";]*/, "annotation"],
      // The symbol right after `(` is in call position.
      [/\(/, { token: "@brackets", next: "@head" }],
      [/[()\[\]{}]/, "@brackets"],
      [/[:~]/, "operator"],
      [
        SYMBOL,
        {
          cases: {
            "@keywords": "keyword",
            [BOOLEAN]: "number",
            [NUMBER]: "number",
            [TYPE]: "type",
            "@default": "identifier",
          },
        },
      ],
    ],
    head: [
      [/\s+/, "white"],
      [
        SYMBOL,
        {
          cases: {
            "@keywords": { token: "keyword", next: "@pop" },
            [NUMBER]: { token: "number", next: "@pop" },
            [TYPE]: { token: "type", next: "@pop" },
            "@default": { token: "function", next: "@pop" },
          },
        },
      ],
      [/./, { token: "@rematch", next: "@pop" }],
    ],
    whitespace: [
      [/\s+/, "white"],
      [/;\*/, { token: "comment", next: "@blockComment" }],
      [/;.*$/, "comment"],
    ],
    blockComment: [
      [/\*;/, { token: "comment", next: "@pop" }],
      [/[^*]+/, "comment"],
      [/\*/, "comment"],
    ],
    string: [
      [/[^\\"]+/, "string"],
      [/\\./, "string.escape"],
      [/"/, { token: "string.quote", next: "@pop" }],
    ],
  },
};

const configuration = {
  // Names can hold most symbols, so double-clicking `mouse-down?` selects
  // all of it.
  wordPattern: /[^\s()\[\]{};"|:~@#]+/,
  comments: { lineComment: ";", blockComment: [";*", "*;"] },
  brackets: [
    ["(", ")"],
    ["[", "]"],
    ["{", "}"],
  ],
  autoClosingPairs: [
    { open: "(", close: ")" },
    { open: "[", close: "]" },
    { open: "{", close: "}" },
    { open: '"', close: '"', notIn: ["string", "comment"] },
  ],
  surroundingPairs: [
    { open: "(", close: ")" },
    { open: "[", close: "]" },
    { open: "{", close: "}" },
    { open: '"', close: '"' },
  ],
  autoCloseBefore: " \t\n)]}",
};

// The docs site's code colors (style.css), light and dark.
const PALETTES = {
  light: {
    base: "vs",
    background: "#fdfcfa",
    foreground: "#26282e",
    lineHighlight: "#f4f1ec",
    selection: "#ede7fa",
    comment: "969288",
    string: "2f7a4d",
    number: "a35b1e",
    keyword: "7048c8",
    type: "1d6d9c",
    call: "45475a",
    paren: "a5a199",
    annotation: "b0388f",
    brackets: ["#7048c8", "#1d6d9c", "#a35b1e", "#2f7a4d", "#b0388f", "#6d6f78"],
  },
  dark: {
    base: "vs-dark",
    background: "#16171c",
    foreground: "#d7dae2",
    lineHighlight: "#1e2027",
    selection: "#3a3260",
    comment: "6d7180",
    string: "7fc79a",
    number: "e0a06a",
    keyword: "a98cf0",
    type: "72b5dd",
    call: "ccd0da",
    paren: "5b5f6b",
    annotation: "dd7ec1",
    brackets: ["#a98cf0", "#72b5dd", "#e0a06a", "#7fc79a", "#dd7ec1", "#ccd0da"],
  },
};

function theme(palette) {
  const p = PALETTES[palette];
  const rule = (token, foreground, fontStyle) => ({
    token,
    foreground,
    ...(fontStyle && { fontStyle }),
  });
  return {
    base: p.base,
    inherit: true,
    rules: [
      rule("comment", p.comment, "italic"),
      rule("string", p.string),
      rule("string.quote", p.string),
      rule("string.escape", p.number),
      rule("number", p.number),
      rule("keyword", p.keyword, "bold"),
      rule("type", p.type),
      rule("function", p.call, "bold"),
      rule("annotation", p.annotation),
      rule("operator", p.paren),
      rule("delimiter", p.paren),
      rule("identifier", p.foreground.slice(1)),
    ],
    colors: {
      "editor.background": p.background,
      "editor.foreground": p.foreground,
      "editor.lineHighlightBackground": p.lineHighlight,
      "editor.selectionBackground": p.selection,
      "editorGutter.background": p.background,
      ...Object.fromEntries(
        p.brackets.map((color, i) => [
          `editorBracketHighlight.foreground${i + 1}`,
          color,
        ]),
      ),
    },
  };
}

/** Registers easl and its themes with `monaco`. */
export function registerEasl(monaco) {
  monaco.languages.register({ id: "easl", extensions: [".easl"] });
  monaco.languages.setMonarchTokensProvider("easl", tokenizer);
  monaco.languages.setLanguageConfiguration("easl", configuration);
  monaco.editor.defineTheme("easl-light", theme("light"));
  monaco.editor.defineTheme("easl-dark", theme("dark"));
}

/** The theme for the system's color scheme, now. */
export function currentTheme() {
  return matchMedia("(prefers-color-scheme: dark)").matches
    ? "easl-dark"
    : "easl-light";
}

/**
 * The indentation for a new line after `before` (the text before the
 * cursor): two past an unclosed `(`, one past an unclosed `[` or `{`, or
 * the previous line's indentation. Strings and comments are skipped.
 */
export function indentAfter(before) {
  const open = [];
  let lineStart = 0;
  for (let i = 0; i < before.length; i++) {
    const c = before[i];
    if (c === "\n") {
      lineStart = i + 1;
    } else if (c === '"') {
      for (i++; i < before.length && before[i] !== '"'; i++) {
        if (before[i] === "\\") i++;
        else if (before[i] === "\n") lineStart = i + 1;
      }
    } else if (c === ";" && before[i + 1] === "*") {
      const end = before.indexOf("*;", i + 2);
      const comment = before.slice(i, end < 0 ? before.length : end);
      const lastBreak = comment.lastIndexOf("\n");
      if (lastBreak >= 0) lineStart = i + lastBreak + 1;
      i = end < 0 ? before.length : end + 1;
    } else if (c === ";") {
      while (i + 1 < before.length && before[i + 1] !== "\n") i++;
    } else if ("([{".includes(c)) {
      open.push({ char: c, column: i - lineStart });
    } else if (")]}".includes(c)) {
      open.pop();
    }
  }
  const innermost = open.at(-1);
  if (innermost) return innermost.column + (innermost.char === "(" ? 2 : 1);
  const lastLine = before.slice(before.lastIndexOf("\n") + 1);
  return lastLine.length - lastLine.trimStart().length;
}

/**
 * The offset of the head of the innermost unclosed `(` in `before` (the
 * text before the cursor): the function a call there calls. Null outside a
 * call, or when the head is the cursor's own word.
 */
export function callHeadBefore(before) {
  const open = [];
  for (let i = 0; i < before.length; i++) {
    const c = before[i];
    if (c === '"') {
      for (i++; i < before.length && before[i] !== '"'; i++) {
        if (before[i] === "\\") i++;
      }
    } else if (c === ";" && before[i + 1] === "*") {
      const end = before.indexOf("*;", i + 2);
      i = end < 0 ? before.length : end + 1;
    } else if (c === ";") {
      while (i + 1 < before.length && before[i + 1] !== "\n") i++;
    } else if ("([{".includes(c)) {
      open.push({ char: c, index: i });
    } else if (")]}".includes(c)) {
      open.pop();
    }
  }
  const innermost = open.at(-1);
  if (!innermost || innermost.char !== "(") return null;
  let head = innermost.index + 1;
  while (head < before.length && /\s/.test(before[head])) head++;
  const word = before.slice(head).match(/^[^\s()\[\]{}";]+/);
  // Still typing the head itself: nothing to show yet.
  if (!word || head + word[0].length === before.length) return null;
  return head;
}
