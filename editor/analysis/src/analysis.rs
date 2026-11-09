//! Compiling one of a sketch's files, with every file it imports, into what
//! the editor shows: diagnostics, inferred types for hover, the names in
//! scope, and where names are defined.
//!
//! This is easl_lsp's `analysis.rs` with the sketch's files in place of the
//! filesystem: files are named by their paths within the sketch
//! (`main.easl`, `lib/noise.easl`), and imports resolve against those paths
//! lexically. As in easl_lsp, each file is analyzed as the main file of its
//! own program, and errors in the files it imports go on the import form
//! that brings them in. Keep the two in step.

use std::{
  collections::{HashMap, VecDeque},
  ops::Range,
  path::{Path, PathBuf},
};

use easl::{
  compiler::{
    builtins::built_in_macros,
    error::{CompileError, ErrorLog},
    info::{definition_signatures, local_references},
    modules::{MainFileIndex, NameKind, WrittenNames},
    program::{CompilerTarget, Program},
    types::{TypeDescription, TypeState, TypeStateDescription},
  },
  parse::{
    EaslMultiDocument, load_and_parse_easl_multidocument_from_sources,
    parse_easl_without_comments,
  },
};
use fsexp::document::DocumentPosition;
use tower_lsp::lsp_types::Range as LspRange;

use crate::source::SourceText;

/// A place in one of the sketch's files.
#[derive(Clone)]
pub struct Location {
  pub path: String,
  pub range: LspRange,
}

pub struct Diagnostic {
  /// Where it goes in the analyzed file.
  pub range: LspRange,
  pub message: String,
  /// Other places the error involves, in any of the sketch's files.
  pub related: Vec<Location>,
}

/// The inferred type of an expression of the analyzed file.
pub struct TypeHint {
  pub span: Range<usize>,
  pub fully_known: bool,
  pub description: String,
}

/// A name in the analyzed file referring to top-level definitions (several
/// for an overloaded function) or to a local binding.
pub struct Reference {
  pub span: Range<usize>,
  /// Where each definition's name is written.
  pub definitions: Vec<Location>,
  /// Each definition's declaration; empty for a local.
  pub signatures: Vec<String>,
}

/// The name of a definition written in the analyzed file.
pub struct DefinitionName {
  pub span: Range<usize>,
  pub signature: Option<String>,
}

pub struct Analysis {
  pub diagnostics: Vec<Diagnostic>,
  pub type_hints: Vec<TypeHint>,
  pub references: Vec<Reference>,
  pub definition_names: Vec<DefinitionName>,
  /// The names the file can refer to, when name resolution got far enough
  /// to know them.
  pub names: Option<Vec<(String, NameKind)>>,
}

/// The outcome of loading a program's files.
enum Loaded {
  /// Some file failed to parse.
  ParseFailures(EaslMultiDocument),
  /// Loading failed for another reason (e.g. an import that can't be found).
  LoadErrors(EaslMultiDocument, ErrorLog),
  Loaded(EaslMultiDocument),
}

/// Loads `path` and everything it imports from `files` (the sketch's files
/// by path).
fn load(path: &str, files: &HashMap<String, String>) -> Loaded {
  let sources: HashMap<PathBuf, String> = files
    .iter()
    .map(|(path, text)| (PathBuf::from(path), text.clone()))
    .collect();
  match load_and_parse_easl_multidocument_from_sources(
    Path::new(path),
    &sources,
  ) {
    Ok(Err(documents)) => Loaded::ParseFailures(documents),
    Ok(Ok(Err((documents, errors)))) => Loaded::LoadErrors(documents, errors),
    Ok(Ok(Ok(documents))) => Loaded::Loaded(documents),
    // The file itself isn't among the sources: compile its text alone.
    Err(_) => {
      let text = files.get(path).cloned().unwrap_or_default();
      let document = parse_easl_without_comments(&text);
      let parse_failed = !document.parsing_failures.is_empty();
      let documents = EaslMultiDocument::from_singular_document(
        document,
        path.to_string(),
        text,
      );
      if parse_failed {
        Loaded::ParseFailures(documents)
      } else {
        Loaded::Loaded(documents)
      }
    }
  }
}

/// Analyzes the file at `path` among `files`.
pub fn analyze(path: &str, files: &HashMap<String, String>) -> Analysis {
  let mut type_hints = vec![];
  let mut names = None;
  let mut index = MainFileIndex::default();
  let mut signatures = vec![];
  let mut local_bindings = vec![];
  let (documents, errors) = match load(path, files) {
    Loaded::ParseFailures(documents) => (documents, ErrorLog::new()),
    Loaded::LoadErrors(documents, errors) => (documents, errors),
    Loaded::Loaded(documents) => {
      let (mut program, mut errors) =
        Program::from_easl_documents(&documents, built_in_macros());
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
      (documents, errors)
    }
  };
  let files = Files::new(&documents);
  let mut diagnostics: Vec<Diagnostic> = errors
    .into_iter()
    .map(|error| files.compile_error_diagnostic(error))
    .collect();
  for (index, (document, _, _)) in documents.sources.iter().enumerate() {
    for failure in document.parsing_failures.iter() {
      diagnostics.push(files.diagnostic(
        format!("{}", failure.kind),
        Some((index, failure.pos.clone())),
        vec![],
      ));
    }
  }
  diagnostics.sort_by_key(|diagnostic| {
    (
      diagnostic.range.start.line,
      diagnostic.range.start.character,
      diagnostic.message.clone(),
    )
  });
  diagnostics.dedup_by(|a, b| a.range == b.range && a.message == b.message);
  // Names are shown the way the file writes them.
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
  let location = |position: &DocumentPosition| {
    files.location(&(*position.path.first()?, position.span.clone()))
  };
  let local_references =
    local_bindings.iter().filter_map(|(position, binding)| {
      (position.path.first() == Some(&0)).then(|| Reference {
        span: position.span.clone(),
        definitions: location(binding).into_iter().collect(),
        signatures: vec![],
      })
    });
  let references = index
    .references
    .iter()
    .filter(|(position, _)| position.path.first() == Some(&0))
    .map(|(position, definitions)| Reference {
      span: position.span.clone(),
      definitions: definitions.iter().filter_map(location).collect(),
      signatures: definitions
        .iter()
        .filter_map(|d| signature_at(&signatures, d))
        .collect(),
    })
    .chain(local_references)
    .filter(|reference| !reference.definitions.is_empty())
    .collect();
  let definition_names = index
    .definitions
    .iter()
    .filter(|position| position.path.first() == Some(&0))
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

/// `text` with each internal name in `written` replaced by how the file
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

/// A span in one of the program's documents, by document index.
type Spot = (usize, Range<usize>);

/// The program's documents, for placing diagnostics. Document 0 is the
/// analyzed file; diagnostics about the others go on the analyzed file's
/// import form that brings them in.
struct Files<'a> {
  documents: &'a EaslMultiDocument,
  sources: Vec<SourceText>,
  /// For each imported document, the span of the analyzed file's import
  /// form through which it's loaded.
  imported_via: HashMap<usize, Range<usize>>,
}

impl<'a> Files<'a> {
  fn new(documents: &'a EaslMultiDocument) -> Self {
    Self {
      documents,
      sources: documents
        .sources
        .iter()
        .map(|(_, _, text)| SourceText::new(text.clone()))
        .collect(),
      imported_via: imported_via(documents),
    }
  }

  /// The document's path within the sketch.
  fn path(&self, index: usize) -> String {
    self.documents.sources[index].1.clone()
  }

  fn location(&self, (index, span): &Spot) -> Option<Location> {
    Some(Location {
      path: self.path(*index),
      range: self.sources.get(*index)?.range(span),
    })
  }

  fn diagnostic(
    &self,
    message: String,
    primary: Option<Spot>,
    secondaries: Vec<Spot>,
  ) -> Diagnostic {
    let mut related = vec![];
    let (range, message) = match primary {
      Some((0, span)) => (self.sources[0].range(&span), message),
      Some(spot) => {
        related.extend(self.location(&spot));
        let range = self
          .imported_via
          .get(&spot.0)
          .map(|span| self.sources[0].range(span))
          .unwrap_or_default();
        (range, format!("in {}: {message}", self.path(spot.0)))
      }
      None => (LspRange::default(), message),
    };
    related.extend(secondaries.iter().filter_map(|spot| self.location(spot)));
    Diagnostic {
      range,
      message,
      related,
    }
  }

  fn compile_error_diagnostic(&self, error: CompileError) -> Diagnostic {
    let spot = |position: DocumentPosition| {
      position.path.first().map(|index| (*index, position.span))
    };
    self.diagnostic(
      error.kind.to_string(),
      error.source_trace.primary_position.and_then(spot),
      error
        .source_trace
        .secondary_positions
        .into_iter()
        .filter_map(spot)
        .collect(),
    )
  }
}

/// For each imported document, the span of the main document's import form
/// through which it's (first) loaded.
fn imported_via(documents: &EaslMultiDocument) -> HashMap<usize, Range<usize>> {
  let mut imports: Vec<(usize, Range<usize>, usize)> = documents
    .imports
    .iter()
    .filter_map(|(position, target)| {
      Some((*position.path.first()?, position.span.clone(), *target))
    })
    .collect();
  imports.sort_by_key(|(from, span, _)| (*from, span.start));
  let mut via: HashMap<usize, Range<usize>> = HashMap::new();
  let mut queue = VecDeque::new();
  for (from, span, target) in imports.iter() {
    if *from == 0 && *target != 0 && !via.contains_key(target) {
      via.insert(*target, span.clone());
      queue.push_back(*target);
    }
  }
  while let Some(document) = queue.pop_front() {
    let span = via[&document].clone();
    for (from, _, target) in imports.iter() {
      if *from == document && *target != 0 && !via.contains_key(target) {
        via.insert(*target, span.clone());
        queue.push_back(*target);
      }
    }
  }
  via
}

/// The `@cpu` entry points defined in `text`: each one's name and the span
/// of its `@cpu` annotation. Read from the text itself (skipping comments
/// and strings), so they're known even while the program has errors.
pub fn cpu_entries(text: &str) -> Vec<(String, Range<usize>)> {
  let bytes = text.as_bytes();
  let mut entries = vec![];
  let mut i = 0;
  let is_delimiter =
    |b: u8| b.is_ascii_whitespace() || b"()[]{}\";".contains(&b);
  let skip_trivia = |mut i: usize| {
    loop {
      while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
      }
      if i < bytes.len() && bytes[i] == b';' {
        while i < bytes.len() && bytes[i] != b'\n' {
          i += 1;
        }
      } else {
        return i;
      }
    }
  };
  while i < bytes.len() {
    match bytes[i] {
      b';' => {
        while i < bytes.len() && bytes[i] != b'\n' {
          i += 1;
        }
      }
      b'"' => {
        i += 1;
        while i < bytes.len() && bytes[i] != b'"' {
          i += if bytes[i] == b'\\' { 2 } else { 1 };
        }
        i += 1;
      }
      b'@'
        if text[i..].starts_with("@cpu")
          && bytes.get(i + 4).is_none_or(|b| is_delimiter(*b)) =>
      {
        let annotation = i..i + 4;
        let mut j = skip_trivia(i + 4);
        if text[j..].starts_with("(defn") {
          j = skip_trivia(j + 5);
          let start = j;
          while j < bytes.len() && !is_delimiter(bytes[j]) {
            j += 1;
          }
          if j > start {
            entries.push((text[start..j].to_string(), annotation));
          }
        }
        i += 4;
      }
      _ => i += 1,
    }
  }
  entries
}
