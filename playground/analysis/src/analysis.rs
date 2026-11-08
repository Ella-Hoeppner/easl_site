//! Compiling the playground's one document into what the editor shows:
//! diagnostics, inferred types for hover, and the names in scope.
//!
//! This is easl_lsp's `analysis.rs` for a single document with no imports
//! (the playground has no files to import), which lets it build for the
//! browser: the original reads imported files from disk and names them by
//! file URL. Keep the two in step.

use std::ops::Range;

use easl::{
  compiler::{
    builtins::built_in_macros,
    error::{CompileError, ErrorLog},
    info::{definition_signatures, local_references},
    modules::{MainFileIndex, NameKind, WrittenNames},
    program::{CompilerTarget, Program},
    types::{TypeDescription, TypeState, TypeStateDescription},
  },
  parse::{EaslMultiDocument, parse_easl_without_comments},
};
use fsexp::document::DocumentPosition;
use tower_lsp::lsp_types::Range as LspRange;

use crate::source::SourceText;

pub struct Diagnostic {
  pub range: LspRange,
  pub message: String,
  /// Other places the error involves, in the same document.
  pub related: Vec<LspRange>,
}

/// The inferred type of an expression.
pub struct TypeHint {
  pub span: Range<usize>,
  pub fully_known: bool,
  pub description: String,
}

/// A name referring to top-level definitions (several for an overloaded
/// function) or to a local binding.
pub struct Reference {
  pub span: Range<usize>,
  /// Where each definition's name is written.
  pub definitions: Vec<Range<usize>>,
  /// Each definition's declaration; empty for a local.
  pub signatures: Vec<String>,
}

/// The name of a definition written in the document.
pub struct DefinitionName {
  pub span: Range<usize>,
  pub signature: Option<String>,
}

pub struct Analysis {
  pub diagnostics: Vec<Diagnostic>,
  pub type_hints: Vec<TypeHint>,
  pub references: Vec<Reference>,
  pub definition_names: Vec<DefinitionName>,
  /// The names the document can refer to, when name resolution got far
  /// enough to know them.
  pub names: Option<Vec<(String, NameKind)>>,
}

pub fn analyze(text: &str) -> Analysis {
  let source = SourceText::new(text.to_string());
  let document = parse_easl_without_comments(text);
  let parse_failures = document.parsing_failures.clone();
  let documents = EaslMultiDocument::from_singular_document(
    document,
    String::new(),
    text.to_string(),
  );
  let mut type_hints = vec![];
  let mut names = None;
  let mut index = MainFileIndex::default();
  let mut signatures = vec![];
  let mut local_bindings = vec![];
  let mut errors = ErrorLog::new();
  if parse_failures.is_empty() {
    let (mut program, resolution_errors) =
      Program::from_easl_documents(&documents, built_in_macros());
    errors = resolution_errors;
    index = program.main_file_index.clone();
    names = Some(
      index
        .names
        .iter()
        .map(|(name, kind)| (name.to_string(), *kind))
        .collect(),
    );
    // Validation rewrites the definitions, so they're described first.
    signatures = definition_signatures(&program, &documents);
    local_bindings = local_references(&program);
    if errors.is_empty() {
      errors = program.validate_raw_program(CompilerTarget::WGSL);
      type_hints = gather_type_hints(&program);
    }
  }
  let range = |span: &Range<usize>| source.range(span);
  let in_document =
    |position: &DocumentPosition| position.path.first() == Some(&0);
  let mut diagnostics: Vec<Diagnostic> = errors
    .into_iter()
    .map(|error: CompileError| {
      let primary = error.source_trace.primary_position;
      Diagnostic {
        range: primary
          .filter(in_document)
          .map(|position| range(&position.span))
          .unwrap_or_default(),
        message: error.kind.to_string(),
        related: error
          .source_trace
          .secondary_positions
          .iter()
          .filter(|position| in_document(position))
          .map(|position| range(&position.span))
          .collect(),
      }
    })
    .collect();
  for failure in parse_failures.iter() {
    diagnostics.push(Diagnostic {
      range: range(&failure.pos),
      message: format!("{}", failure.kind),
      related: vec![],
    });
  }
  diagnostics.sort_by_key(|diagnostic| {
    (
      diagnostic.range.start.line,
      diagnostic.range.start.character,
      diagnostic.message.clone(),
    )
  });
  diagnostics.dedup_by(|a, b| a.range == b.range && a.message == b.message);
  // Names are shown the way the document writes them.
  let written = |text: &str| written_names(text, &index.written_names);
  for diagnostic in diagnostics.iter_mut() {
    diagnostic.message = written(&diagnostic.message);
  }
  for hint in type_hints.iter_mut() {
    hint.description = written(&hint.description);
  }
  for (_, signature) in signatures.iter_mut() {
    *signature = written(signature);
  }
  let local_references =
    local_bindings.iter().filter_map(|(position, binding)| {
      (in_document(position) && in_document(binding)).then(|| Reference {
        span: position.span.clone(),
        definitions: vec![binding.span.clone()],
        signatures: vec![],
      })
    });
  let references = index
    .references
    .iter()
    .filter(|(position, _)| in_document(position))
    .map(|(position, definitions)| {
      let definitions: Vec<&DocumentPosition> =
        definitions.iter().filter(|d| in_document(d)).collect();
      Reference {
        span: position.span.clone(),
        definitions: definitions.iter().map(|d| d.span.clone()).collect(),
        signatures: definitions
          .iter()
          .filter_map(|d| signature_at(&signatures, d))
          .collect(),
      }
    })
    .chain(local_references)
    .filter(|reference| !reference.definitions.is_empty())
    .collect();
  let definition_names = index
    .definitions
    .iter()
    .filter(|position| in_document(position))
    .map(|position| DefinitionName {
      span: position.span.clone(),
      signature: signature_at(&signatures, position),
    })
    .collect();
  Analysis {
    diagnostics,
    type_hints,
    references,
    definition_names,
    names,
  }
}

/// `text` with each internal name in `written` replaced by how the document
/// writes it. Internal names that differ from their written names contain
/// `/`, so ordinary words are never replaced.
fn written_names(text: &str, written: &WrittenNames) -> String {
  if written.is_empty() {
    return text.to_string();
  }
  let mut result = String::with_capacity(text.len());
  let mut token = String::new();
  let flush = |token: &mut String, result: &mut String| {
    match written.get(token.as_str()) {
      Some(name) => result.push_str(name),
      None => result.push_str(token),
    }
    token.clear();
  };
  for c in text.chars() {
    if c.is_whitespace() || "()[]{}`\"',:".contains(c) {
      flush(&mut token, &mut result);
      result.push(c);
    } else {
      token.push(c);
    }
  }
  flush(&mut token, &mut result);
  result
}

/// The declaration of the definition whose name is at `position`: the
/// signature keyed by the smallest span containing it (a variant's key is
/// the variant, which contains its name).
fn signature_at(
  signatures: &[(DocumentPosition, String)],
  position: &DocumentPosition,
) -> Option<String> {
  signatures
    .iter()
    .filter(|(key, _)| {
      key.path.first() == position.path.first()
        && key.span.start <= position.span.start
        && position.span.end <= key.span.end
    })
    .min_by_key(|(key, _)| key.span.len())
    .map(|(_, signature)| signature.clone())
}

fn gather_type_hints(program: &Program) -> Vec<TypeHint> {
  program
    .gather_type_annotations()
    .into_iter()
    .filter_map(|(source_trace, typestate)| {
      let position = source_trace.primary_position?;
      (position.path.first() == Some(&0)).then(|| TypeHint {
        span: position.span,
        fully_known: typestate.check_is_fully_known(),
        description: match typestate {
          TypeState::Known(t) => TypeDescription::from(t).to_string(),
          other => format!("?? {}", TypeStateDescription::from(other)),
        },
      })
    })
    .collect()
}
