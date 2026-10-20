//! Reader for Valve's KeyValues text format ("VDF"): enough for the
//! `toolmanifest.vdf` / `compatibilitytool.vdf` a compatibility tool ships.
//!
//! ```text
//! "manifest"
//! {
//!   "version" "2"
//!   "commandline" "/proton %verb%"   // a comment
//!   "require_tool_appid" "1628350"
//! }
//! ```
//!
//! Keys are case-insensitive like in Steam; a repeated key keeps the last
//! value; `[$CONDITION]` tags and `#base` / `#include` lines are ignored.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Map(Map),
}

/// Keys are stored lower-cased.
pub type Map = BTreeMap<String, Value>;

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(m) => m.get(&key.to_ascii_lowercase()),
            Value::Str(_) => None,
        }
    }

    /// String value of a key below this map.
    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Value::Str(s) => Some(s),
            Value::Map(_) => None,
        }
    }

    #[cfg(test)]
    pub fn as_map(&self) -> Option<&Map> {
        match self {
            Value::Map(m) => Some(m),
            Value::Str(_) => None,
        }
    }
}

/// Parse a whole document; the top level is a map of its root keys.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut tokens = Tokenizer {
        chars: text.chars().peekable(),
    };
    let mut root = Map::new();
    parse_body(&mut tokens, &mut root, true, 0)?;
    Ok(Value::Map(root))
}

/// Deeper nesting than any tool manifest has; keeps a crafted file from
/// exhausting the stack (the parser recurses per `{`).
const MAX_DEPTH: usize = 64;

/// Read a `toolmanifest.vdf` and return its `manifest` block.
pub fn load_manifest(path: &std::path::Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text)?
        .get("manifest")
        .cloned()
        .ok_or_else(|| format!("{}: no \"manifest\" block", path.display()))
}

#[derive(Debug, PartialEq)]
enum Token {
    Str(String),
    Open,
    Close,
}

struct Tokenizer<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl Tokenizer<'_> {
    fn next(&mut self) -> Result<Option<Token>, String> {
        loop {
            let Some(&c) = self.chars.peek() else {
                return Ok(None);
            };
            if c.is_whitespace() {
                self.chars.next();
                continue;
            }
            if c == '/' {
                self.chars.next();
                if self.chars.peek() == Some(&'/') {
                    for c in self.chars.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                    continue;
                }
                return Ok(Some(Token::Str(format!("/{}", self.bare()))));
            }
            if c == '{' {
                self.chars.next();
                return Ok(Some(Token::Open));
            }
            if c == '}' {
                self.chars.next();
                return Ok(Some(Token::Close));
            }
            if c == '"' {
                self.chars.next();
                return Ok(Some(Token::Str(self.quoted()?)));
            }
            // conditionals like [$WIN32] and #base / #include lines carry nothing we need
            if c == '[' {
                for c in self.chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                }
                continue;
            }
            if c == '#' {
                for c in self.chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
                continue;
            }
            return Ok(Some(Token::Str(self.bare())));
        }
    }

    fn quoted(&mut self) -> Result<String, String> {
        let mut out = String::new();
        loop {
            match self.chars.next() {
                None => return Err("unterminated string".into()),
                Some('"') => return Ok(out),
                Some('\\') => match self.chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some(c) => out.push(c),
                    None => return Err("unterminated string".into()),
                },
                Some(c) => out.push(c),
            }
        }
    }

    fn bare(&mut self) -> String {
        let mut out = String::new();
        while let Some(&c) = self.chars.peek() {
            if c.is_whitespace() || matches!(c, '{' | '}' | '"') {
                break;
            }
            out.push(c);
            self.chars.next();
        }
        out
    }
}

fn parse_body(
    tokens: &mut Tokenizer,
    into: &mut Map,
    top: bool,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("nested deeper than {MAX_DEPTH} levels"));
    }
    loop {
        let key = match tokens.next()? {
            None if top => return Ok(()),
            None => return Err("unexpected end of file, missing }".into()),
            Some(Token::Close) if !top => return Ok(()),
            Some(Token::Close) => return Err("unexpected }".into()),
            Some(Token::Open) => return Err("unexpected {".into()),
            Some(Token::Str(k)) => k.to_ascii_lowercase(),
        };
        match tokens.next()? {
            Some(Token::Str(v)) => {
                into.insert(key, Value::Str(v));
            }
            Some(Token::Open) => {
                let mut child = Map::new();
                parse_body(tokens, &mut child, false, depth + 1)?;
                into.insert(key, Value::Map(child));
            }
            Some(Token::Close) => return Err(format!("key \"{key}\" has no value")),
            None => return Err(format!("key \"{key}\" has no value")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_proton_toolmanifest() {
        let text = r#"// Generated file, do not edit
"manifest"
{
  "version" "2"
  "commandline" "/proton %verb%"
  "require_tool_appid" "1628350"
  "use_sessions" "1"
  "compatmanager_layer_name" "proton"
}
"#;
        let doc = parse(text).expect("parses");
        let m = doc.get("manifest").expect("manifest");
        assert_eq!(m.str("commandline"), Some("/proton %verb%"));
        assert_eq!(m.str("require_tool_appid"), Some("1628350"));
        assert_eq!(m.str("Compatmanager_Layer_Name"), Some("proton"));
        assert_eq!(m.str("missing"), None);
    }

    #[test]
    fn reads_nested_maps_bare_words_and_conditionals() {
        let text = r#"
"compatibilitytools"
{
    "compat_tools"
    {
        "UMU-Proton-10.0-4" // Internal name
        {
            install_path   .
            "display_name" "UMU-Proton 10.0-4" [$LINUX]
            "escaped" "a \"b\"\tc"
        }
    }
}
"#;
        let doc = parse(text).expect("parses");
        let tools = doc
            .get("compatibilitytools")
            .and_then(|v| v.get("compat_tools"))
            .and_then(Value::as_map)
            .expect("tools");
        let (name, tool) = tools.iter().next().expect("one tool");
        assert_eq!(name, "umu-proton-10.0-4");
        assert_eq!(tool.str("install_path"), Some("."));
        assert_eq!(tool.str("display_name"), Some("UMU-Proton 10.0-4"));
        assert_eq!(tool.str("escaped"), Some("a \"b\"\tc"));
    }

    #[test]
    fn rejects_broken_input() {
        assert!(parse("\"a\" {").is_err());
        assert!(parse("\"a\"").is_err());
        assert!(parse("}").is_err());
        assert!(parse("\"a\" \"unterminated").is_err());
        assert_eq!(parse("").expect("empty is fine"), Value::Map(Map::new()));
    }

    #[test]
    fn nesting_is_capped() {
        let deep = |n: usize| format!("{}{}", "\"k\" {".repeat(n), "}".repeat(n));
        assert!(parse(&deep(MAX_DEPTH)).is_ok());
        assert!(parse(&deep(MAX_DEPTH + 1)).is_err());
        assert!(parse(&deep(10_000)).is_err());
    }
}
