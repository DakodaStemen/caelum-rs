//! Syntax highlighting: syntect parses, and the app theme supplies every color.
use crate::theme::Theme;
use eframe::egui::{
    Color32, FontId,
    text::{LayoutJob, TextFormat},
};
use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    ops::Range,
    path::Path,
};
use syntect::{
    parsing::{ParseState, Scope, ScopeStack, SyntaxSet},
    util::LinesWithEndings,
};
/// Theme `syntax` keys, in the order of [`Kind`].
pub const KINDS: &[&str] = &[
    "variable",
    "comment",
    "string",
    "number",
    "keyword",
    "function",
    "type",
    "constant",
    "operator",
    "punctuation",
    "tag",
    "attribute",
    "escape",
    "parameter",
];
const MAX_BYTES: usize = 600_000;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Plain,
    Comment,
    Str,
    Number,
    Keyword,
    Function,
    Type,
    Constant,
    Operator,
    Punct,
    Tag,
    Attribute,
    Escape,
    Param,
}
impl Kind {
    fn key(self) -> &'static str {
        KINDS[self as usize]
    }
}
fn classify(scope: &str) -> Option<Kind> {
    let starts = |p: &str| scope.starts_with(p);
    Some(if starts("comment") {
        Kind::Comment
    } else if starts("constant.character.escape") || starts("constant.other.placeholder") {
        Kind::Escape
    } else if starts("string") {
        Kind::Str
    } else if starts("constant.numeric") {
        Kind::Number
    } else if starts("constant") || starts("support.constant") || starts("variable.language") {
        Kind::Constant
    } else if starts("keyword.operator") {
        Kind::Operator
    } else if starts("keyword") || starts("storage") {
        Kind::Keyword
    } else if starts("entity.name.function")
        || starts("support.function")
        || starts("variable.function")
        || starts("entity.name.command")
    {
        Kind::Function
    } else if starts("entity.name.tag") {
        Kind::Tag
    } else if starts("entity.other.attribute-name") || starts("meta.annotation") {
        Kind::Attribute
    } else if starts("entity.name")
        || starts("entity.other.inherited-class")
        || starts("support.type")
        || starts("support.class")
    {
        Kind::Type
    } else if starts("variable.parameter") {
        Kind::Param
    } else if starts("markup.heading") {
        Kind::Keyword
    } else if starts("punctuation.definition.comment") {
        Kind::Comment
    } else if starts("punctuation") {
        Kind::Punct
    } else {
        return None;
    })
}
struct Spans {
    key: u64,
    spans: Vec<(Range<usize>, Kind)>,
}
pub struct Highlighter {
    set: SyntaxSet,
    memo: HashMap<Scope, Option<Kind>>,
    cache: Option<Spans>,
}
impl Default for Highlighter {
    fn default() -> Self {
        Self {
            set: SyntaxSet::load_defaults_newlines(),
            memo: HashMap::new(),
            cache: None,
        }
    }
}
impl Highlighter {
    /// Name of the language recognised for a path, if any.
    pub fn language(&self, path: &Path) -> Option<String> {
        self.syntax_for(path, "").map(|s| s.name.clone())
    }
    fn syntax_for(
        &self,
        path: &Path,
        first_line: &str,
    ) -> Option<&syntect::parsing::SyntaxReference> {
        let name = path.file_name()?.to_string_lossy();
        let ext = path.extension().map(|e| e.to_string_lossy().to_string());
        let by_ext = |e: &str| self.set.find_syntax_by_extension(e);
        let found = by_ext(&name)
            .or_else(|| ext.as_deref().and_then(by_ext))
            .or_else(|| match ext.as_deref() {
                Some("toml" | "ini" | "cfg" | "conf" | "lock") => by_ext("properties"),
                Some("ts" | "tsx" | "jsx" | "mjs" | "cjs") => by_ext("js"),
                Some("zsh" | "bash") => by_ext("sh"),
                Some("yml") => by_ext("yaml"),
                _ => None,
            })
            .or_else(|| self.set.find_syntax_by_first_line(first_line));
        found.filter(|s| s.name != "Plain Text")
    }
    fn spans(&mut self, path: &Path, text: &str) -> &[(Range<usize>, Kind)] {
        let mut h = DefaultHasher::new();
        (path, text).hash(&mut h);
        let key = h.finish();
        if self.cache.as_ref().is_none_or(|c| c.key != key) {
            let spans = self.parse(path, text);
            self.cache = Some(Spans { key, spans });
        }
        &self.cache.as_ref().unwrap().spans
    }
    fn parse(&mut self, path: &Path, text: &str) -> Vec<(Range<usize>, Kind)> {
        let plain = vec![(0..text.len(), Kind::Plain)];
        if text.len() > MAX_BYTES {
            return plain;
        }
        let first = text.lines().next().unwrap_or("");
        let Some(syntax) = self.syntax_for(path, first) else {
            return plain;
        };
        let mut state = ParseState::new(syntax);
        let mut stack = ScopeStack::new();
        let mut spans: Vec<(Range<usize>, Kind)> = Vec::new();
        let mut offset = 0;
        for line in LinesWithEndings::from(text) {
            let Ok(ops) = state.parse_line(line, &self.set) else {
                return plain;
            };
            let mut at = 0;
            let push =
                |spans: &mut Vec<(Range<usize>, Kind)>, from: usize, to: usize, kind: Kind| {
                    if to <= from {
                        return;
                    }
                    let (a, b) = (offset + from, offset + to);
                    match spans.last_mut() {
                        Some((r, k)) if *k == kind && r.end == a => r.end = b,
                        _ => spans.push((a..b, kind)),
                    }
                };
            for (pos, op) in ops {
                let kind = Self::stack_kind(&mut self.memo, &stack);
                push(&mut spans, at, pos, kind);
                at = pos;
                if stack.apply(&op).is_err() {
                    return plain;
                }
            }
            let kind = Self::stack_kind(&mut self.memo, &stack);
            push(&mut spans, at, line.len(), kind);
            offset += line.len();
        }
        spans
    }
    fn stack_kind(memo: &mut HashMap<Scope, Option<Kind>>, stack: &ScopeStack) -> Kind {
        for scope in stack.as_slice().iter().rev() {
            let found = *memo
                .entry(*scope)
                .or_insert_with(|| classify(&scope.build_string()));
            if let Some(kind) = found {
                return kind;
            }
        }
        Kind::Plain
    }
    /// Builds the colored layout for the editor; plain text when the language is unknown.
    pub fn job(
        &mut self,
        path: &Path,
        text: &str,
        theme: &Theme,
        font: FontId,
        wrap: f32,
    ) -> LayoutJob {
        let mut job = LayoutJob::default();
        let _ = wrap; // code scrolls sideways instead of wrapping
        job.wrap.max_width = f32::INFINITY;
        let spans = self.spans(path, text).to_vec();
        let mut colors: HashMap<&'static str, Color32> = HashMap::new();
        for (range, kind) in spans {
            let color = *colors
                .entry(kind.key())
                .or_insert_with(|| theme.syn(kind.key()));
            let Some(piece) = text.get(range) else {
                continue;
            };
            job.append(piece, 0., TextFormat::simple(font.clone(), color));
        }
        if job.sections.is_empty() {
            job.append(text, 0., TextFormat::simple(font, theme.syn("variable")));
        }
        job
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rust_tokens_are_colored_distinctly() {
        let mut h = Highlighter::default();
        let spans = h
            .spans(
                Path::new("a.rs"),
                "fn main() { let x = \"hi\"; // note\n}\n",
            )
            .to_vec();
        let kinds: Vec<Kind> = spans.iter().map(|s| s.1).collect();
        for k in [Kind::Keyword, Kind::Function, Kind::Str, Kind::Comment] {
            assert!(kinds.contains(&k), "{k:?} missing in {kinds:?}");
        }
        assert_eq!(spans.last().unwrap().0.end, 36);
    }
    #[test]
    fn unknown_language_stays_plain() {
        let mut h = Highlighter::default();
        assert!(h.language(Path::new("notes.zzz")).is_none());
        assert_eq!(h.spans(Path::new("notes.zzz"), "hello").len(), 1);
    }
    #[test]
    fn common_languages_are_recognised() {
        let h = Highlighter::default();
        for f in [
            "a.py", "a.js", "a.ts", "a.json", "a.toml", "a.sh", "a.md", "a.html", "a.css", "a.yml",
            "Makefile",
        ] {
            assert!(h.language(Path::new(f)).is_some(), "{f}");
        }
    }
}
