//! Static site generator for the easl documentation.
//!
//! Reads the Markdown pages in `pages/`, converts them to HTML with
//! pulldown-cmark (plus a hand-rolled easl syntax highlighter), and writes a
//! self-contained static site to `site/`. It also generates an
//! alphabetical, filterable index of every builtin function, collected from
//! the `###` headings of the reference pages.
//!
//! Usage: `cargo run` from `generator/`.

use pulldown_cmark::{
  html::push_html, CodeBlockKind, CowStr, Event, HeadingLevel, Options,
  Parser, Tag, TagEnd,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use live_preview::LivePreview;

mod highlight;
mod landing;
mod live_preview;
mod nested_build;
mod playground;

/// One page of the site, in nav order.
struct PageSpec {
  /// path of the markdown source, relative to `pages/`
  source: &'static str,
  /// title shown in the sidebar
  nav_title: &'static str,
  /// which sidebar section the page belongs to
  section: &'static str,
  /// the playground menu entry listing the page's live previews (as Part 1,
  /// Part 2, …), when it isn't `nav_title`
  playground_menu: Option<&'static str>,
}

const PAGES: &[PageSpec] = &[
  PageSpec { source: "guide.md", nav_title: "Overview", section: "Guide", playground_menu: None },
  PageSpec { source: "language.md", nav_title: "Language guide", section: "Guide", playground_menu: None },
  PageSpec { source: "shaders.md", nav_title: "Writing shaders", section: "Guide", playground_menu: None },
  PageSpec { source: "cpu.md", nav_title: "The CPU runtime", section: "Guide", playground_menu: None },
  PageSpec { source: "raymarch.md", nav_title: "Ray marching", section: "Walkthroughs", playground_menu: Some("Raymarcher") },
  PageSpec { source: "reference/overview.md", nav_title: "Builtin reference", section: "Reference", playground_menu: None },
  PageSpec { source: "reference/operators.md", nav_title: "Operators", section: "Reference", playground_menu: None },
  PageSpec { source: "reference/math.md", nav_title: "Math functions", section: "Reference", playground_menu: None },
  PageSpec { source: "reference/vectors.md", nav_title: "Vectors", section: "Reference", playground_menu: None },
  PageSpec { source: "reference/matrices.md", nav_title: "Matrices", section: "Reference", playground_menu: None },
  PageSpec { source: "reference/conversions.md", nav_title: "Conversions & bits", section: "Reference", playground_menu: None },
  PageSpec { source: "reference/arrays.md", nav_title: "Arrays", section: "Reference", playground_menu: None },
  PageSpec { source: "reference/textures.md", nav_title: "Textures", section: "Reference", playground_menu: None },
  PageSpec { source: "reference/gpu-builtins.md", nav_title: "GPU builtins", section: "Reference", playground_menu: None },
  PageSpec { source: "reference/cpu-builtins.md", nav_title: "CPU builtins", section: "Reference", playground_menu: None },
];

/// The sidebar section the playground's link closes.
const PLAYGROUND_SECTION: &str = "Walkthroughs";

/// The generated function-index page; gets a nav entry at the end of the
/// Reference section.
const FUNCTION_INDEX_OUTPUT: &str = "functions.html";

struct Heading {
  level: HeadingLevel,
  text: String,
  slug: String,
}

struct RenderedPage {
  output: String, // path relative to site root, e.g. "reference/math.html"
  title: String,  // from the h1
  body: String,
  headings: Vec<Heading>,
  previews: Vec<LivePreview>,
}

/// One entry in the all-functions index.
struct FnEntry {
  name: String,
  page_output: String,
  page_title: String,
  slug: String,
}

fn main() {
  let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
  let pages_dir = manifest_dir.join("../pages");
  let site_dir = manifest_dir.join("../site");

  let mut rendered: Vec<RenderedPage> = Vec::new();
  let mut fn_entries: Vec<FnEntry> = Vec::new();

  for spec in PAGES {
    let source_path = pages_dir.join(spec.source);
    let source = fs::read_to_string(&source_path)
      .unwrap_or_else(|e| panic!("couldn't read {}: {e}", source_path.display()));
    let output = spec.source.replace(".md", ".html");
    let is_reference_page = spec.source.starts_with("reference/")
      && spec.source != "reference/overview.md";
    let page_dir = Path::new(spec.source).parent().unwrap();
    let page = render_page(
      &source,
      &output,
      page_dir,
      is_reference_page,
      &mut fn_entries,
    );
    rendered.push(page);
  }

  fn_entries.sort_by(|a, b| {
    sort_key(&a.name)
      .cmp(&sort_key(&b.name))
      .then_with(|| a.page_output.cmp(&b.page_output))
  });

  fs::create_dir_all(site_dir.join("reference")).unwrap();
  fs::write(site_dir.join("style.css"), include_str!("../assets/style.css"))
    .unwrap();

  let runtime_dir = site_dir.join(landing::RUNTIME_DIR);
  fs::create_dir_all(&runtime_dir).unwrap();
  for page in &rendered {
    for preview in &page.previews {
      preview.package(&pages_dir, &runtime_dir);
    }
    let html = wrap_in_template(
      &page.title,
      &page.output,
      &page.body,
      &rendered,
      &page.headings,
      !page.previews.is_empty(),
    );
    fs::write(site_dir.join(&page.output), html).unwrap();
  }
  fs::write(
    site_dir.join("live-previews.js"),
    include_str!("../assets/live-previews.js"),
  )
  .unwrap();

  let index_body = function_index_body(&fn_entries);
  let index_html = wrap_in_template(
    "Function index",
    FUNCTION_INDEX_OUTPUT,
    &index_body,
    &rendered,
    &[],
    false,
  );
  fs::write(site_dir.join(FUNCTION_INDEX_OUTPUT), index_html).unwrap();

  landing::generate(&site_dir);

  // The playground offers the landing page's program, then the programs in
  // `pages/playground/`, then each page's live previews, numbered in one
  // submenu per page.
  let mut examples = vec![landing::hero_example()];
  examples.extend(playground::extra_examples());
  for (spec, page) in PAGES.iter().zip(&rendered) {
    let menu = spec.playground_menu.unwrap_or(spec.nav_title);
    for (i, preview) in page.previews.iter().enumerate() {
      examples.push(preview.example(&pages_dir, menu, i + 1));
    }
  }
  playground::generate(&site_dir, examples, landing::HERO_EXAMPLE);

  let distinct_names = fn_entries
    .iter()
    .map(|e| e.name.as_str())
    .collect::<HashSet<_>>()
    .len();
  println!(
    "generated {} pages + function index ({} distinct functions, {} entries)\n-> {}",
    rendered.len(),
    distinct_names,
    fn_entries.len(),
    site_dir.canonicalize().unwrap().display()
  );
}

/// Convert one markdown page into HTML, collecting headings (for the sidebar
/// table of contents), reference-function entries, and live previews along
/// the way. `page_dir` is the page's directory relative to `pages/`.
fn render_page(
  source: &str,
  output: &str,
  page_dir: &Path,
  collect_fn_entries: bool,
  fn_entries: &mut Vec<FnEntry>,
) -> RenderedPage {
  let parser = Parser::new_ext(source, Options::ENABLE_TABLES);
  let mut events: Vec<Event> = Vec::new();
  let mut headings: Vec<Heading> = Vec::new();
  let mut title = String::new();
  let mut used_slugs: HashSet<String> = HashSet::new();
  let mut section_counter = 0usize;
  // names already indexed for the current h3 anchor, so an explicit
  // `<!-- index: ... -->` comment doesn't duplicate the heading's code spans
  let mut last_h3_indexed: HashSet<String> = HashSet::new();
  let mut last_h3_slug = String::new();
  let mut previews: Vec<LivePreview> = Vec::new();
  let prefix = page_prefix(output);

  let mut iter = parser.into_iter();
  while let Some(event) = iter.next() {
    match event {
      Event::Start(Tag::Heading { level, classes, attrs, .. }) => {
        // gather the heading's inner events to derive its text and slug
        let mut inner: Vec<Event> = Vec::new();
        let mut text = String::new();
        let mut code_spans: Vec<String> = Vec::new();
        for inner_event in iter.by_ref() {
          match &inner_event {
            Event::Text(t) => text.push_str(t),
            Event::Code(c) => {
              text.push_str(c);
              code_spans.push(c.to_string());
            }
            Event::End(TagEnd::Heading(_)) => break,
            _ => {}
          }
          inner.push(inner_event);
        }
        let mut slug = github_slug(&text);
        if slug.is_empty() {
          slug = code_spans
            .first()
            .map(|s| symbol_slug(s))
            .unwrap_or_default();
        }
        if slug.is_empty() {
          section_counter += 1;
          slug = format!("sec-{section_counter}");
        }
        while used_slugs.contains(&slug) {
          slug.push_str("-x");
        }
        used_slugs.insert(slug.clone());
        if level == HeadingLevel::H1 && title.is_empty() {
          title = text.clone();
        }
        if collect_fn_entries && level == HeadingLevel::H3 {
          last_h3_indexed.clear();
          last_h3_slug = slug.clone();
          for name in &code_spans {
            let name = strip_parenthetical(name);
            if last_h3_indexed.insert(name.clone()) {
              fn_entries.push(FnEntry {
                name,
                page_output: output.to_string(),
                page_title: String::new(), // filled in once the h1 is known
                slug: slug.clone(),
              });
            }
          }
        }
        headings.push(Heading { level, text, slug: slug.clone() });
        events.push(Event::Start(Tag::Heading {
          level,
          id: Some(CowStr::from(slug.clone())),
          classes,
          attrs,
        }));
        events.extend(inner);
        if level != HeadingLevel::H1 {
          events.push(Event::InlineHtml(CowStr::from(format!(
            "<a class=\"hlink\" href=\"#{slug}\" aria-label=\"link to this section\">#</a>"
          ))));
        }
        events.push(Event::End(TagEnd::Heading(level)));
      }
      Event::Start(Tag::CodeBlock(kind)) => {
        let lang = match &kind {
          CodeBlockKind::Fenced(lang) => lang.to_string(),
          CodeBlockKind::Indented => String::new(),
        };
        let mut code = String::new();
        for inner_event in iter.by_ref() {
          match inner_event {
            Event::Text(t) => code.push_str(&t),
            Event::End(TagEnd::CodeBlock) => break,
            _ => {}
          }
        }
        let mut words = lang.split_whitespace();
        let lang = words.next().unwrap_or("");
        let no_playground = words.any(|word| word == "no-playground");
        let block = if lang == "easl" {
          // A whole program gets a link that opens it in the playground,
          // unless its fence says `easl no-playground`.
          let link = if !no_playground && playground::is_runnable(&code) {
            format!(
              "<p class=\"code-playground\"><a href=\"{}\">Open in the \
               playground</a></p>\n",
              playground::code_link(prefix, &code)
            )
          } else {
            String::new()
          };
          format!(
            "<pre class=\"code easl\"><code>{}</code></pre>\n{link}",
            highlight::highlight_easl(&code)
          )
        } else if lang == "glsl" {
          format!(
            "<pre class=\"code glsl\"><code>{}</code></pre>\n",
            highlight::highlight_glsl(&code)
          )
        } else {
          format!(
            "<pre class=\"code\"><code>{}</code></pre>\n",
            escape_html(&code)
          )
        };
        events.push(Event::Html(CowStr::from(block)));
      }
      Event::Start(Tag::Link { link_type, dest_url, title, id }) => {
        events.push(Event::Start(Tag::Link {
          link_type,
          dest_url: rewrite_md_link(&dest_url),
          title,
          id,
        }));
      }
      Event::Html(html) | Event::InlineHtml(html)
        if html.trim_start().starts_with("<!-- index:") =>
      {
        if collect_fn_entries {
          let inner = html
            .trim()
            .trim_start_matches("<!-- index:")
            .trim_end_matches("-->")
            .trim();
          for name in inner.split_whitespace() {
            if last_h3_indexed.insert(name.to_string()) {
              fn_entries.push(FnEntry {
                name: name.to_string(),
                page_output: output.to_string(),
                page_title: String::new(),
                slug: last_h3_slug.clone(),
              });
            }
          }
        }
        // the comment itself is dropped from the output
      }
      Event::Html(html) => match LivePreview::from_marker(&html, page_dir) {
        Some(preview) => {
          events.push(Event::Html(CowStr::from(preview.html(prefix))));
          previews.push(preview);
        }
        None => events.push(Event::Html(html)),
      },
      other => events.push(other),
    }
  }

  let mut body = String::new();
  push_html(&mut body, events.into_iter());

  let page = RenderedPage {
    output: output.to_string(),
    title: if title.is_empty() { output.to_string() } else { title },
    body,
    headings,
    previews,
  };
  for entry in fn_entries.iter_mut() {
    if entry.page_output == page.output && entry.page_title.is_empty() {
      entry.page_title = page.title.clone();
    }
  }
  page
}

/// Rewrite relative links between markdown files into links between the
/// generated html files. External links pass through untouched.
fn rewrite_md_link<'a>(dest: &CowStr<'a>) -> CowStr<'a> {
  if dest.starts_with("http://") || dest.starts_with("https://") {
    return dest.clone();
  }
  if let Some(hash) = dest.find('#') {
    let (path, fragment) = dest.split_at(hash);
    if path.ends_with(".md") {
      return CowStr::from(format!(
        "{}.html{}",
        &path[..path.len() - 3],
        fragment
      ));
    }
  } else if dest.ends_with(".md") {
    return CowStr::from(format!("{}.html", &dest[..dest.len() - 3]));
  }
  dest.clone()
}

/// GitHub-style heading slugs: lowercase, spaces to hyphens, all other
/// punctuation dropped (existing hyphens and underscores survive).
fn github_slug(text: &str) -> String {
  let mut slug: String = text
    .to_lowercase()
    .chars()
    .filter_map(|c| {
      if c.is_alphanumeric() || c == '-' || c == '_' {
        Some(c)
      } else if c == ' ' {
        Some('-')
      } else {
        None
      }
    })
    .collect();
  // headings made only of symbols slugify to pure hyphens; treat as empty so
  // the symbol_slug fallback kicks in
  if slug.chars().all(|c| c == '-') {
    return String::new();
  }
  while slug.starts_with('-') {
    slug.remove(0);
  }
  while slug.ends_with('-') {
    slug.pop();
  }
  slug
}

/// Readable anchors for operator headings: `+=` becomes "op-plus-eq".
fn symbol_slug(symbol: &str) -> String {
  let mut parts: Vec<&str> = vec![];
  for c in symbol.chars() {
    parts.push(match c {
      '+' => "plus",
      '-' => "minus",
      '*' => "times",
      '/' => "div",
      '%' => "mod",
      '=' => "eq",
      '<' => "lt",
      '>' => "gt",
      '!' => "bang",
      '&' => "amp",
      '|' => "pipe",
      '^' => "caret",
      _ => continue,
    });
  }
  if parts.is_empty() {
    String::new()
  } else {
    format!("op-{}", parts.join("-"))
  }
}

/// "`+`, `-` (matrices)" index entries shouldn't include the parenthetical.
fn strip_parenthetical(name: &str) -> String {
  name.split(" (").next().unwrap_or(name).trim().to_string()
}

/// Sort operators ahead of named functions, then alphabetically.
fn sort_key(name: &str) -> (u8, String) {
  let alphabetic = name.chars().next().is_some_and(|c| c.is_alphanumeric());
  (if alphabetic { 1 } else { 0 }, name.to_lowercase())
}

fn function_index_body(entries: &[FnEntry]) -> String {
  let mut items = String::new();
  for entry in entries {
    items.push_str(&format!(
      "<li data-name=\"{name}\"><a href=\"{page}#{slug}\"><code>{name}</code></a><span class=\"fn-page\">{page_title}</span></li>\n",
      name = escape_html(&entry.name),
      page = entry.page_output,
      slug = entry.slug,
      page_title = escape_html(&entry.page_title),
    ));
  }
  format!(
    "<h1 id=\"function-index\">Function index</h1>\n\
     <p>Every builtin function, with operators first. Type to filter; click through for signatures and details.</p>\n\
     <input type=\"search\" id=\"fn-filter\" placeholder=\"filter functions…\" autocomplete=\"off\" autofocus>\n\
     <ul class=\"fn-index\" id=\"fn-list\">\n{items}</ul>\n\
     <p id=\"fn-none\" hidden>No matching functions.</p>\n\
     <script>\n\
     const filter = document.getElementById('fn-filter');\n\
     const items = Array.from(document.querySelectorAll('#fn-list li'));\n\
     const none = document.getElementById('fn-none');\n\
     filter.addEventListener('input', () => {{\n\
       const q = filter.value.trim().toLowerCase();\n\
       let shown = 0;\n\
       for (const li of items) {{\n\
         const show = li.dataset.name.toLowerCase().includes(q);\n\
         li.hidden = !show;\n\
         if (show) shown++;\n\
       }}\n\
       none.hidden = shown > 0;\n\
     }});\n\
     </script>\n"
  )
}

fn wrap_in_template(
  title: &str,
  output: &str,
  body: &str,
  all_pages: &[RenderedPage],
  headings: &[Heading],
  has_previews: bool,
) -> String {
  let prefix = page_prefix(output);
  let scripts = if has_previews {
    format!("<script type=\"module\" src=\"{prefix}live-previews.js\"></script>\n")
  } else {
    String::new()
  };

  let mut nav = String::new();
  let mut current_section = "";
  for (spec, page) in PAGES.iter().zip(all_pages) {
    if spec.section != current_section {
      if current_section == PLAYGROUND_SECTION {
        nav.push_str(&format!(
          "<li><a href=\"{prefix}{}\">Playground</a></li>\n",
          playground::PLAYGROUND_PAGE
        ));
      }
      if !current_section.is_empty() {
        nav.push_str("</ul>\n");
      }
      nav.push_str(&format!(
        "<h2 class=\"nav-section\">{}</h2>\n<ul>\n",
        spec.section
      ));
      current_section = spec.section;
    }
    let is_current = page.output == output;
    nav.push_str(&format!(
      "<li{current}><a href=\"{prefix}{href}\">{title}</a>\n",
      current = if is_current { " class=\"current\"" } else { "" },
      href = page.output,
      title = spec.nav_title,
    ));
    if is_current {
      let subheadings: Vec<&Heading> = headings
        .iter()
        .filter(|h| h.level == HeadingLevel::H2)
        .collect();
      if !subheadings.is_empty() {
        nav.push_str("<ul class=\"toc\">\n");
        for h in subheadings {
          nav.push_str(&format!(
            "<li><a href=\"#{}\">{}</a></li>\n",
            h.slug,
            escape_html(&h.text)
          ));
        }
        nav.push_str("</ul>\n");
      }
    }
    nav.push_str("</li>\n");
  }
  // the generated function index lives at the end of the last (Reference)
  // section, whose <ul> is still open here
  nav.push_str(&format!(
    "<li{current}><a href=\"{prefix}{FUNCTION_INDEX_OUTPUT}\">Function index</a></li>\n</ul>\n",
    current = if output == FUNCTION_INDEX_OUTPUT {
      " class=\"current\""
    } else {
      ""
    },
  ));

  let full_title = if title.eq_ignore_ascii_case("easl") {
    "Easl".to_string()
  } else {
    format!("{title} — Easl")
  };
  format!(
    "<!doctype html>\n\
     <html lang=\"en\">\n\
     <head>\n\
     <meta charset=\"utf-8\">\n\
     <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
     <title>{title}</title>\n\
     <link rel=\"stylesheet\" href=\"{prefix}style.css\">\n\
     </head>\n\
     <body>\n\
     <nav class=\"sidebar\">\n\
     <div class=\"logo\"><a href=\"{prefix}index.html\">easl</a>\
     <span class=\"tagline\">enhanced abstraction shader language</span></div>\n\
     {nav}\
     </nav>\n\
     <main><article>\n{body}</article></main>\n\
     {scripts}\
     </body>\n\
     </html>\n",
    title = escape_html(&full_title),
  )
}

/// The relative path from the page at `output` back to the site root.
fn page_prefix(output: &str) -> &'static str {
  if output.contains('/') { "../" } else { "" }
}

pub fn escape_html(text: &str) -> String {
  text
    .replace('&', "&amp;")
    .replace('<', "&lt;")
    .replace('>', "&gt;")
    .replace('"', "&quot;")
}
