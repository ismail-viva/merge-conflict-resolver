//! Syntax highlighting with tree-sitter.
//!
//! A file is parsed once with its language's grammar and the grammar's own
//! highlight query; the captures are folded into a small, theme-independent
//! set of [`Token`] kinds and stored per line. Which color a token gets is
//! entirely the theme's business.

use std::sync::OnceLock;

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// Inputs larger than this are shown without highlighting.
pub const MAX_BYTES: usize = 1 << 20;

macro_rules! tokens {
    ($( $(#[$doc:meta])* $variant:ident => $name:literal ),* $(,)?) => {
        /// What a piece of source text is, as far as coloring goes.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Token { $( $(#[$doc])* $variant, )* }

        impl Token {
            pub const ALL: &'static [Token] = &[ $( Token::$variant, )* ];

            /// The name of the token's color in a theme's `syntax` table.
            pub fn name(self) -> &'static str {
                match self { $( Token::$variant => $name, )* }
            }
        }
    };
}

tokens! {
    /// Declaring keywords: `def`, `fn`, `const`, `class`.
    Keyword => "keyword",
    /// Control flow and imports: `return`, `if`, `for`, `import`.
    KeywordControl => "keyword_control",
    Function => "function",
    FunctionBuiltin => "function_builtin",
    /// Types, classes, constructors and UI components.
    Type => "type",
    TypeBuiltin => "type_builtin",
    /// Numbers, booleans and named constants.
    Constant => "constant",
    String => "string",
    /// Escapes and interpolation delimiters inside strings.
    Escape => "escape",
    Comment => "comment",
    /// Fields, object keys and markup attributes.
    Property => "property",
    Parameter => "parameter",
    Variable => "variable",
    /// `self`, `this`, `super`.
    VariableBuiltin => "variable_builtin",
    Operator => "operator",
    Punctuation => "punctuation",
    /// Markup tags: `<section>`.
    Tag => "tag",
    /// Decorators and attributes: `@dataclass`, `#[derive]`.
    Attribute => "attribute",
}

impl Token {
    /// Tokens conventionally set in italics.
    pub fn is_italic(self) -> bool {
        matches!(self, Token::Comment | Token::KeywordControl)
    }
}

/// Keywords that steer control flow or pull in other modules.
const CONTROL_KEYWORDS: &[&str] = &[
    "return",
    "if",
    "else",
    "elif",
    "for",
    "while",
    "in",
    "break",
    "continue",
    "match",
    "case",
    "switch",
    "default",
    "try",
    "except",
    "catch",
    "finally",
    "raise",
    "throw",
    "yield",
    "await",
    "async",
    "with",
    "as",
    "import",
    "from",
    "export",
    "use",
    "pub",
    "mod",
    "package",
    "go",
    "defer",
    "select",
    "loop",
    "do",
    "then",
    "fi",
    "done",
    "esac",
    "pass",
    "assert",
    "del",
    "where",
    "goto",
    "fallthrough",
    "range",
    "and",
    "or",
    "not",
    "is",
];

/// Additions to the grammars' own queries, for distinctions an editor is
/// expected to show but the bundled queries leave out.
const PYTHON_EXTRA: &str = r#"
(parameters (identifier) @variable.parameter)
(default_parameter name: (identifier) @variable.parameter)
(typed_parameter (identifier) @variable.parameter)
(typed_default_parameter name: (identifier) @variable.parameter)
(lambda_parameters (identifier) @variable.parameter)
"#;

const JAVASCRIPT_EXTRA: &str = r#"
(formal_parameters (identifier) @variable.parameter)
(formal_parameters
  (object_pattern (shorthand_property_identifier_pattern) @variable.parameter))
(arrow_function parameter: (identifier) @variable.parameter)
"#;

const TYPESCRIPT_EXTRA: &str = r#"
(required_parameter pattern: (identifier) @variable.parameter)
(optional_parameter pattern: (identifier) @variable.parameter)
(required_parameter
  pattern: (object_pattern (shorthand_property_identifier_pattern) @variable.parameter))
(arrow_function parameter: (identifier) @variable.parameter)
"#;

/// Capture names understood in highlight queries, with the token they map to.
/// tree-sitter matches a capture to the longest listed prefix of its name.
const CAPTURES: &[(&str, Option<Token>)] = &[
    ("attribute", Some(Token::Attribute)),
    ("boolean", Some(Token::Constant)),
    ("charset", Some(Token::KeywordControl)),
    ("comment", Some(Token::Comment)),
    ("constant", Some(Token::Constant)),
    ("constructor", Some(Token::Type)),
    ("embedded", None),
    ("escape", Some(Token::Escape)),
    ("function", Some(Token::Function)),
    ("function.builtin", Some(Token::FunctionBuiltin)),
    ("import", Some(Token::KeywordControl)),
    ("keyframes", Some(Token::KeywordControl)),
    ("keyword", Some(Token::Keyword)),
    ("label", Some(Token::Keyword)),
    ("media", Some(Token::KeywordControl)),
    ("module", Some(Token::Type)),
    ("namespace", Some(Token::Type)),
    ("number", Some(Token::Constant)),
    ("operator", Some(Token::Operator)),
    ("property", Some(Token::Property)),
    ("punctuation", Some(Token::Punctuation)),
    ("punctuation.special", Some(Token::Escape)),
    ("string", Some(Token::String)),
    ("string.escape", Some(Token::Escape)),
    ("string.special.key", Some(Token::Property)),
    ("supports", Some(Token::KeywordControl)),
    ("tag", Some(Token::Tag)),
    ("text.literal", Some(Token::String)),
    ("text.reference", Some(Token::Property)),
    ("text.title", Some(Token::Function)),
    ("text.uri", Some(Token::Constant)),
    ("type", Some(Token::Type)),
    ("type.builtin", Some(Token::TypeBuiltin)),
    ("variable", Some(Token::Variable)),
    ("variable.builtin", Some(Token::VariableBuiltin)),
    ("variable.parameter", Some(Token::Parameter)),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Python,
    Rust,
    JavaScript,
    TypeScript,
    Tsx,
    Go,
    Json,
    Bash,
    Html,
    Css,
    Toml,
    Yaml,
    Markdown,
}

impl Language {
    const ALL: [Language; 13] = [
        Language::Python,
        Language::Rust,
        Language::JavaScript,
        Language::TypeScript,
        Language::Tsx,
        Language::Go,
        Language::Json,
        Language::Bash,
        Language::Html,
        Language::Css,
        Language::Toml,
        Language::Yaml,
        Language::Markdown,
    ];

    /// The language of a file, judged by its name.
    pub fn from_path(path: &str) -> Option<Language> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let extension = name
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase());
        Some(match extension.as_deref() {
            Some("py" | "pyi" | "pyw") => Language::Python,
            Some("rs") => Language::Rust,
            Some("js" | "mjs" | "cjs" | "jsx") => Language::JavaScript,
            Some("ts" | "mts" | "cts") => Language::TypeScript,
            Some("tsx") => Language::Tsx,
            Some("go") => Language::Go,
            Some("json" | "jsonc") => Language::Json,
            Some("sh" | "bash" | "zsh") => Language::Bash,
            Some("html" | "htm") => Language::Html,
            Some("css") => Language::Css,
            Some("toml") => Language::Toml,
            Some("yaml" | "yml") => Language::Yaml,
            Some("md" | "markdown") => Language::Markdown,
            _ => match name {
                "Cargo.lock" | "uv.lock" | "poetry.lock" => Language::Toml,
                ".bashrc" | ".zshrc" | ".profile" => Language::Bash,
                _ => return None,
            },
        })
    }

    /// In markup-flavored languages an `attribute` is a tag's property, not
    /// a decorator.
    fn attributes_are_properties(self) -> bool {
        matches!(
            self,
            Language::JavaScript | Language::TypeScript | Language::Tsx | Language::Html
        )
    }

    fn build(self) -> Option<HighlightConfiguration> {
        let javascript = tree_sitter_javascript::HIGHLIGHT_QUERY;
        let jsx = tree_sitter_javascript::JSX_HIGHLIGHT_QUERY;
        let typescript = tree_sitter_typescript::HIGHLIGHTS_QUERY;
        let (language, highlights, locals): (tree_sitter::Language, String, &str) = match self {
            Language::Python => (
                tree_sitter_python::LANGUAGE.into(),
                format!("{}\n{PYTHON_EXTRA}", tree_sitter_python::HIGHLIGHTS_QUERY),
                "",
            ),
            Language::Rust => (
                tree_sitter_rust::LANGUAGE.into(),
                tree_sitter_rust::HIGHLIGHTS_QUERY.to_owned(),
                "",
            ),
            Language::JavaScript => (
                tree_sitter_javascript::LANGUAGE.into(),
                format!("{javascript}\n{jsx}\n{JAVASCRIPT_EXTRA}"),
                tree_sitter_javascript::LOCALS_QUERY,
            ),
            // The TypeScript queries only add to the JavaScript ones.
            Language::TypeScript => (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                format!("{javascript}\n{typescript}\n{TYPESCRIPT_EXTRA}"),
                tree_sitter_typescript::LOCALS_QUERY,
            ),
            Language::Tsx => (
                tree_sitter_typescript::LANGUAGE_TSX.into(),
                format!("{javascript}\n{jsx}\n{typescript}\n{TYPESCRIPT_EXTRA}"),
                tree_sitter_typescript::LOCALS_QUERY,
            ),
            Language::Go => (
                tree_sitter_go::LANGUAGE.into(),
                tree_sitter_go::HIGHLIGHTS_QUERY.to_owned(),
                "",
            ),
            Language::Json => (
                tree_sitter_json::LANGUAGE.into(),
                tree_sitter_json::HIGHLIGHTS_QUERY.to_owned(),
                "",
            ),
            Language::Bash => (
                tree_sitter_bash::LANGUAGE.into(),
                tree_sitter_bash::HIGHLIGHT_QUERY.to_owned(),
                "",
            ),
            Language::Html => (
                tree_sitter_html::LANGUAGE.into(),
                tree_sitter_html::HIGHLIGHTS_QUERY.to_owned(),
                "",
            ),
            Language::Css => (
                tree_sitter_css::LANGUAGE.into(),
                tree_sitter_css::HIGHLIGHTS_QUERY.to_owned(),
                "",
            ),
            Language::Toml => (
                tree_sitter_toml_ng::LANGUAGE.into(),
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY.to_owned(),
                "",
            ),
            Language::Yaml => (
                tree_sitter_yaml::LANGUAGE.into(),
                tree_sitter_yaml::HIGHLIGHTS_QUERY.to_owned(),
                "",
            ),
            Language::Markdown => (
                tree_sitter_md::LANGUAGE.into(),
                tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.to_owned(),
                "",
            ),
        };
        let mut config =
            HighlightConfiguration::new(language, format!("{self:?}"), &highlights, "", locals)
                .ok()?;
        let names: Vec<&str> = CAPTURES.iter().map(|(name, _)| *name).collect();
        config.configure(&names);
        Some(config)
    }

    /// The language's highlighting setup, built on first use.
    fn config(self) -> Option<&'static HighlightConfiguration> {
        static CONFIGS: [OnceLock<Option<HighlightConfiguration>>; 13] =
            [const { OnceLock::new() }; 13];
        let index = Language::ALL.iter().position(|l| *l == self)?;
        CONFIGS[index].get_or_init(|| self.build()).as_ref()
    }
}

/// A highlighted stretch of one line; offsets are bytes from the line start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub token: Token,
}

/// The tokens of a text, line by line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Highlighted {
    lines: Vec<Vec<Span>>,
}

impl Highlighted {
    /// Tokens of the `index`-th line, in order; empty when there are none.
    pub fn line(&self, index: usize) -> &[Span] {
        self.lines.get(index).map_or(&[], Vec::as_slice)
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// Highlight `text`. Returns `None` when the text is too large or cannot be
/// processed; the caller then shows it plain.
pub fn highlight(language: Language, text: &str) -> Option<Highlighted> {
    if text.len() > MAX_BYTES {
        return None;
    }
    let config = language.config()?;
    let mut highlighter = Highlighter::new();
    let events = highlighter
        .highlight(config, text.as_bytes(), None, |_| None)
        .ok()?;

    // Byte offset at which every line starts.
    let mut line_starts = vec![0usize];
    line_starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    if line_starts.last() == Some(&text.len()) {
        line_starts.pop();
    }
    let mut lines: Vec<Vec<Span>> = vec![Vec::new(); line_starts.len()];

    let mut stack: Vec<Option<Token>> = Vec::new();
    let mut line = 0usize;
    for event in events {
        match event.ok()? {
            HighlightEvent::HighlightStart(highlight) => {
                stack.push(CAPTURES.get(highlight.0).and_then(|(_, token)| *token));
            }
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                // The innermost capture that means something wins.
                let Some(token) = stack.iter().rev().find_map(|t| *t) else {
                    continue;
                };
                let token = refine(language, token, &text[start..end]);
                // A token may span lines (strings, comments): cut it up.
                let mut from = start;
                while from < end {
                    while line + 1 < line_starts.len() && line_starts[line + 1] <= from {
                        line += 1;
                    }
                    let line_end = line_starts.get(line + 1).copied().unwrap_or(text.len());
                    let to = end.min(line_end);
                    let base = line_starts[line];
                    // Some queries capture a whole node, padding included;
                    // whitespace only matters inside strings and comments.
                    let (mut lo, mut hi) = (from, to);
                    if !matches!(token, Token::String | Token::Comment) {
                        let piece = &text[from..to];
                        lo += piece.len() - piece.trim_start().len();
                        hi -= piece.len() - piece.trim_end().len();
                    }
                    if lo < hi {
                        lines[line].push(Span {
                            start: (lo - base) as u32,
                            end: (hi - base) as u32,
                            token,
                        });
                    }
                    from = to;
                }
            }
        }
    }
    Some(Highlighted { lines })
}

/// Distinctions the grammars' queries do not make themselves.
fn refine(language: Language, token: Token, text: &str) -> Token {
    match token {
        // Word operators (`in`, `and`, `not`) read as keywords.
        Token::Keyword | Token::Operator if CONTROL_KEYWORDS.contains(&text) => {
            Token::KeywordControl
        }
        Token::Attribute if language.attributes_are_properties() => Token::Property,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(text, token)` for every highlighted stretch of the first line.
    fn tokens(language: Language, text: &str) -> Vec<(String, Token)> {
        let highlighted = highlight(language, text).expect("highlighting works");
        let first = text.split_inclusive('\n').next().unwrap_or("");
        highlighted
            .line(0)
            .iter()
            .map(|s| (first[s.start as usize..s.end as usize].to_owned(), s.token))
            .collect()
    }

    fn has(tokens: &[(String, Token)], text: &str, token: Token) -> bool {
        tokens.iter().any(|(t, k)| t == text && *k == token)
    }

    #[test]
    fn every_language_has_a_working_grammar() {
        for language in Language::ALL {
            assert!(language.config().is_some(), "{language:?} failed to load");
        }
    }

    #[test]
    fn languages_are_detected_from_file_names() {
        for (path, language) in [
            ("src/app/main.py", Language::Python),
            ("lib.rs", Language::Rust),
            ("web/Cart.tsx", Language::Tsx),
            ("web/Cart.JSX", Language::JavaScript),
            ("api.ts", Language::TypeScript),
            ("cmd/main.go", Language::Go),
            ("package.json", Language::Json),
            ("deploy.sh", Language::Bash),
            ("Cargo.toml", Language::Toml),
            ("Cargo.lock", Language::Toml),
            ("ci.yml", Language::Yaml),
            ("README.md", Language::Markdown),
        ] {
            assert_eq!(Language::from_path(path), Some(language), "{path}");
        }
        assert_eq!(Language::from_path("notes.txt"), None);
        assert_eq!(Language::from_path("Makefile"), None);
    }

    #[test]
    fn python_tokens() {
        let t = tokens(Language::Python, "def total(items, tax=0.2):  # sum\n");
        assert!(has(&t, "def", Token::Keyword), "{t:?}");
        assert!(has(&t, "total", Token::Function), "{t:?}");
        assert!(has(&t, "0.2", Token::Constant), "{t:?}");
        assert!(has(&t, "items", Token::Parameter), "{t:?}");
        assert!(has(&t, "tax", Token::Parameter), "{t:?}");
        assert!(has(&t, "# sum", Token::Comment), "{t:?}");

        let t = tokens(Language::Python, "for item in items:\n");
        assert!(has(&t, "for", Token::KeywordControl), "{t:?}");
        assert!(has(&t, "in", Token::KeywordControl), "{t:?}");

        let t = tokens(
            Language::Python,
            "    return f\"{item.name}: {round(x)}\"\n",
        );
        assert!(has(&t, "return", Token::KeywordControl), "{t:?}");
        assert!(t.iter().any(|(_, k)| *k == Token::String), "{t:?}");
        assert!(has(&t, "name", Token::Property), "{t:?}");

        let t = tokens(Language::Python, "@dataclass\nclass Item:\n");
        assert!(t.iter().any(|(text, _)| text == "dataclass"), "{t:?}");
    }

    #[test]
    fn rust_tokens() {
        let t = tokens(
            Language::Rust,
            "pub fn area(r: f64) -> Option<String> { None }\n",
        );
        assert!(has(&t, "fn", Token::Keyword), "{t:?}");
        assert!(has(&t, "pub", Token::KeywordControl), "{t:?}");
        assert!(has(&t, "area", Token::Function), "{t:?}");
        assert!(has(&t, "f64", Token::TypeBuiltin), "{t:?}");
        assert!(has(&t, "String", Token::Type), "{t:?}");
    }

    #[test]
    fn tsx_distinguishes_tags_components_and_attributes() {
        let t = tokens(
            Language::Tsx,
            "const a = <section className=\"x\"><Badge count={3} /></section>;\n",
        );
        assert!(has(&t, "const", Token::Keyword), "{t:?}");
        assert!(has(&t, "section", Token::Tag), "{t:?}");
        assert!(has(&t, "Badge", Token::Type), "{t:?}");
        assert!(has(&t, "className", Token::Property), "{t:?}");
        assert!(has(&t, "\"x\"", Token::String), "{t:?}");
        assert!(has(&t, "3", Token::Constant), "{t:?}");
    }

    #[test]
    fn typescript_tokens() {
        let t = tokens(
            Language::TypeScript,
            "export function sum(a: number): Total { return a }\n",
        );
        assert!(has(&t, "export", Token::KeywordControl), "{t:?}");
        assert!(has(&t, "function", Token::Keyword), "{t:?}");
        assert!(has(&t, "sum", Token::Function), "{t:?}");
        assert!(has(&t, "a", Token::Parameter), "{t:?}");
        assert!(has(&t, "number", Token::TypeBuiltin), "{t:?}");
        assert!(has(&t, "Total", Token::Type), "{t:?}");

        let t = tokens(
            Language::Tsx,
            "export function Cart({ items, onPay }: Props) {\n",
        );
        // A capitalized function is a component: it takes the type color.
        assert!(has(&t, "Cart", Token::Type), "{t:?}");
        assert!(has(&t, "items", Token::Parameter), "{t:?}");
        assert!(has(&t, "Props", Token::Type), "{t:?}");

        let t = tokens(
            Language::JavaScript,
            "function add(a, { b }) { return a + b }\n",
        );
        assert!(
            has(&t, "a", Token::Parameter) && has(&t, "b", Token::Parameter),
            "{t:?}"
        );
    }

    #[test]
    fn data_formats_highlight_keys_and_values() {
        let t = tokens(
            Language::Json,
            "{\"name\": \"mcr\", \"n\": 3, \"ok\": true}\n",
        );
        assert!(has(&t, "3", Token::Constant), "{t:?}");
        assert!(t.iter().any(|(_, k)| *k == Token::String), "{t:?}");

        let t = tokens(Language::Toml, "name = \"mcr\" # tool\n");
        assert!(has(&t, "name", Token::Property), "{t:?}");
        assert!(has(&t, "\"mcr\"", Token::String), "{t:?}");
        assert!(has(&t, "# tool", Token::Comment), "{t:?}");
    }

    #[test]
    fn multi_line_tokens_are_cut_at_line_ends() {
        let text = "x = \"\"\"one\ntwo\"\"\"\ny = 1\n";
        let highlighted = highlight(Language::Python, text).unwrap();
        assert_eq!(highlighted.len(), 3);
        let string_on = |line: usize| {
            highlighted
                .line(line)
                .iter()
                .any(|s| s.token == Token::String)
        };
        assert!(string_on(0) && string_on(1) && !string_on(2));
        // Offsets are relative to each line and stay inside it.
        for (index, line) in text.split_inclusive('\n').enumerate() {
            for span in highlighted.line(index) {
                assert!(span.start < span.end && span.end as usize <= line.len());
            }
        }
    }

    #[test]
    fn broken_code_and_odd_inputs_never_fail() {
        for text in [
            "",
            "def (:\n  ]]] \"unterminated",
            "\n\n\n",
            "é = 'ü'\r\nx\r\n",
            "no newline",
        ] {
            let highlighted = highlight(Language::Python, text).expect("still highlights");
            assert_eq!(
                highlighted.len(),
                text.split_inclusive('\n').count(),
                "{text:?}"
            );
        }
        assert!(highlight(Language::Python, &"x\n".repeat(MAX_BYTES)).is_none());
    }

    #[test]
    fn token_names_are_unique() {
        let mut names: Vec<_> = Token::ALL.iter().map(|t| t.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Token::ALL.len());
    }
}
