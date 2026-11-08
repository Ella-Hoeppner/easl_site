//! The playground editor's language support, for the browser: what easl_lsp
//! gives VS Code, for one document. The page runs this in a worker, so a
//! slow (or stuck) compilation never freezes the editor.
//!
//! Most of the work is easl_lsp's own code, included unchanged from the
//! sibling `easl_lsp` repository: positions (`source.rs`), semantic tokens
//! and identifiers (`syntax.rs`), completion candidates (`completion.rs`),
//! and structural edits (`structural.rs`). `analysis.rs` is its analysis
//! for a single document. The request handling mirrors easl_lsp's
//! `server.rs`.
//!
//! Positions are LSP positions: zero-based lines and UTF-16 columns.
//! Results are JSON strings.

use std::{cell::RefCell, ops::Range, panic};

use easl::{format::format_document, parse::parse_easl};
use js_sys::{Function, Reflect};
use serde_json::{Value, json};
use tower_lsp::lsp_types::{Position, Range as LspRange};
use wasm_bindgen::prelude::*;

#[allow(dead_code)]
#[path = "../../../../easl_lsp/src/source.rs"]
mod source;

#[allow(dead_code)]
#[path = "../../../../easl_lsp/src/syntax.rs"]
mod syntax;

#[allow(dead_code)]
#[path = "../../../../easl_lsp/src/completion.rs"]
mod completion;

#[allow(dead_code)]
#[path = "../../../../easl_lsp/src/structural.rs"]
mod structural;

mod analysis;

use analysis::{Analysis, analyze as analyze_text};
use source::SourceText;

/// The last analysis, which hover and go-to-definition answer from (the
/// page only asks them about the text it was computed from).
struct Analyzed {
  source: SourceText,
  analysis: Analysis,
}

thread_local! {
  static LAST: RefCell<Option<Analyzed>> = const { RefCell::new(None) };
}

/// Installs the panic hook: a compiler panic ends this instance (panics
/// abort in the browser), so its message goes to the page's
/// `easlAnalysisPanic` function, if it defines one, before the instance
/// dies.
#[wasm_bindgen(start)]
pub fn start() {
  panic::set_hook(Box::new(|info| {
    let message = info.to_string();
    let global = js_sys::global();
    if let Ok(report) = Reflect::get(&global, &"easlAnalysisPanic".into())
      && let Some(report) = report.dyn_ref::<Function>()
    {
      let _ = report.call1(&JsValue::NULL, &message.into());
    }
  }));
}

fn range_json(range: LspRange) -> Value {
  json!([
    range.start.line,
    range.start.character,
    range.end.line,
    range.end.character
  ])
}

/// Compiles `text`, keeping the result for hover and go-to-definition, and
/// returns its diagnostics: `[{range, message, related: [range]}]`.
#[wasm_bindgen]
pub fn analyze(text: String) -> String {
  let analysis = analyze_text(&text);
  let diagnostics: Vec<Value> = analysis
    .diagnostics
    .iter()
    .map(|diagnostic| {
      json!({
        "range": range_json(diagnostic.range),
        "message": diagnostic.message,
        "related": diagnostic.related.iter().copied().map(range_json)
          .collect::<Vec<_>>(),
      })
    })
    .collect();
  LAST.with(|last| {
    *last.borrow_mut() = Some(Analyzed {
      source: SourceText::new(text),
      analysis,
    })
  });
  Value::Array(diagnostics).to_string()
}

/// The innermost item whose span contains `offset`.
fn innermost<T>(
  items: &[T],
  offset: usize,
  span: impl Fn(&T) -> &Range<usize>,
) -> Option<&T> {
  items
    .iter()
    .filter(|item| span(item).contains(&offset))
    .min_by_key(|item| span(item).len())
}

/// Runs `f` on the last analysis and the byte offset of a position in its
/// text, or returns `"null"`.
fn at_position(
  line: u32,
  character: u32,
  f: impl FnOnce(&Analyzed, usize) -> Option<Value>,
) -> String {
  LAST
    .with(|last| {
      let last = last.borrow();
      let analyzed = last.as_ref()?;
      let offset = analyzed.source.offset(Position { line, character })?;
      f(analyzed, offset)
    })
    .unwrap_or(Value::Null)
    .to_string()
}

/// What hovering a position shows, as in easl_lsp: a name's declarations
/// (each overload's), a definition's own declaration, or else the type of
/// the innermost expression there. `{range, contents: [markdown]}` or null.
#[wasm_bindgen]
pub fn hover(line: u32, character: u32) -> String {
  at_position(line, character, |analyzed, offset| {
    let analysis = &analyzed.analysis;
    let hover = |span: &Range<usize>, declarations: Vec<&str>| {
      Some(json!({
        "range": range_json(analyzed.source.range(span)),
        "contents": declarations
          .iter()
          .map(|declaration| format!("```easl\n{declaration}\n```"))
          .collect::<Vec<_>>(),
      }))
    };
    if let Some(reference) =
      innermost(&analysis.references, offset, |r| &r.span)
    {
      let mut declarations: Vec<&str> =
        reference.signatures.iter().map(String::as_str).collect();
      declarations.dedup();
      if !declarations.is_empty() {
        return hover(&reference.span, declarations);
      }
    }
    if let Some(name) =
      innermost(&analysis.definition_names, offset, |name| &name.span)
      && let Some(signature) = &name.signature
    {
      return hover(&name.span, vec![signature]);
    }
    let hint = analysis
      .type_hints
      .iter()
      .filter(|hint| hint.span.contains(&offset))
      .min_by_key(|hint| (hint.span.len(), !hint.fully_known))?;
    hover(&hint.span, vec![&hint.description])
  })
}

/// Where the name at a position is defined: `[range]` (several for an
/// overloaded function), or null.
#[wasm_bindgen]
pub fn definition(line: u32, character: u32) -> String {
  at_position(line, character, |analyzed, offset| {
    let reference =
      innermost(&analyzed.analysis.references, offset, |r| &r.span)?;
    Some(Value::Array(
      reference
        .definitions
        .iter()
        .map(|span| range_json(analyzed.source.range(span)))
        .collect(),
    ))
  })
}

/// Every completion candidate for a cursor in `text`, as easl_lsp offers
/// them (the editor filters): `[{label, kind, sortText, range}]`, where
/// `kind` is the LSP completion item kind and `range` is the word the
/// chosen item replaces.
#[wasm_bindgen]
pub fn completions(text: String, line: u32, character: u32) -> String {
  let source = SourceText::new(text);
  let Some(cursor) = source.offset(Position { line, character }) else {
    return "[]".to_string();
  };
  let text = source.text();
  let token_start = text[..cursor]
    .char_indices()
    .rev()
    .find(|(_, c)| c.is_whitespace() || "()[]{};\"".contains(*c))
    .map_or(0, |(i, c)| i + c.len_utf8());
  let replaced = LspRange {
    start: source.position(token_start),
    end: Position { line, character },
  };
  let names = LAST.with(|last| {
    last
      .borrow()
      .as_ref()
      .and_then(|analyzed| analyzed.analysis.names.clone())
      .unwrap_or_default()
  });
  let items = completion::completion_items(
    &text[token_start..cursor],
    replaced,
    &names,
    syntax::identifiers(text),
  );
  Value::Array(
    items
      .into_iter()
      .map(|item| {
        json!({
          "label": item.label,
          "kind": item.kind,
          "sortText": item.sort_text,
          "range": range_json(replaced),
        })
      })
      .collect(),
  )
  .to_string()
}

/// `text` in the easl formatter's canonical layout, or undefined when it
/// doesn't parse.
#[wasm_bindgen]
pub fn format(text: String) -> Option<String> {
  format_document(parse_easl(&text)).ok()
}

/// The semantic token types, in legend order (see `semantic_tokens`).
#[wasm_bindgen(js_name = semanticTokenTypes)]
pub fn semantic_token_types() -> String {
  json!(
    syntax::token_legend()
      .token_types
      .iter()
      .map(|token_type| token_type.as_str())
      .collect::<Vec<_>>()
  )
  .to_string()
}

/// `text`'s semantic tokens, in the LSP's relative encoding: five numbers
/// per token (line delta, start delta, length, type, modifiers).
#[wasm_bindgen(js_name = semanticTokens)]
pub fn semantic_tokens(text: String) -> Vec<u32> {
  syntax::semantic_tokens(&SourceText::new(text))
    .into_iter()
    .flat_map(|token| {
      [
        token.delta_line,
        token.delta_start,
        token.length,
        token.token_type,
        token.token_modifiers_bitset,
      ]
    })
    .collect()
}

/// easl_lsp's structural editing commands, on the selection from
/// (`start_line`, `start_character`) to (`end_line`, `end_character`):
///
/// - `expandSelection`: the enclosing form's range, `[sl, sc, el, ec]`
/// - `moveCursorToStart` / `moveCursorToEnd`: `[line, character]`
/// - `moveFormForward` / `moveFormBackward`: `{range, newText, selection}`,
///   where `selection` is in the edited text
///
/// Each returns null when there's nothing to do.
#[wasm_bindgen]
pub fn structural(
  command: &str,
  text: String,
  start_line: u32,
  start_character: u32,
  end_line: u32,
  end_character: u32,
) -> String {
  let source = SourceText::new(text);
  let offset = |line, character| source.offset(Position { line, character });
  let (Some(start), Some(end)) = (
    offset(start_line, start_character),
    offset(end_line, end_character),
  ) else {
    return "null".to_string();
  };
  let selection = start..end;
  let document = parse_easl(source.text());
  let position_json =
    |position: Position| json!([position.line, position.character]);
  let result = match command {
    "expandSelection" => document
      .expand_selection(&selection)
      .map(|expanded| range_json(source.range(&expanded))),
    "moveCursorToStart" => Some(position_json(
      source.position(document.move_cursor_to_start(&selection)),
    )),
    "moveCursorToEnd" => Some(position_json(
      source.position(document.move_cursor_to_end(&selection)),
    )),
    "moveFormForward" | "moveFormBackward" => structural::move_form(
      source.text(),
      &document.syntax_trees,
      &selection,
      command == "moveFormForward",
    )
    .map(|form_move| {
      let mut edited = source.text().to_string();
      edited.replace_range(form_move.replaced.clone(), &form_move.new_text);
      json!({
        "range": range_json(source.range(&form_move.replaced)),
        "newText": form_move.new_text,
        "selection":
          range_json(SourceText::new(edited).range(&form_move.selection)),
      })
    }),
    _ => None,
  };
  result.unwrap_or(Value::Null).to_string()
}
