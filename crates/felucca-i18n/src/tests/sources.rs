//! Message discovery in the workspace sources, used by the catalog tests.

use std::{fs, path::Path};

/// Calls whose first string argument is a localizable message, with the path
/// spelled the way the sources spell it.
const MESSAGE_CALLS: &[&str] = &[
    "Message::text",
    "Message::with",
    "ConfigError::invalid",
    "bad",
    "say",
    "Error::Invalid",
    "Error::Conflict",
    "Error::Unavailable",
    // The admin listener's `Error(StatusCode, "…".into())` form.
    "Error",
];

/// Every message literal in the sibling crates, as `(crate, file, line, text)`.
///
/// The scan is deliberately small: it understands the Rust string literal,
/// escapes, line continuations and comments, which is all these call sites use.
/// A message a translator would miss is a failing test, not a silent fallback.
pub fn messages() -> Vec<(String, String, usize, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut found = Vec::new();
    for member in [
        "felucca-core",
        "felucca-proto",
        "felucca-mux",
        "felucca-transport",
        "felucca-service",
        "felucca-server",
        "felucca-client",
        "felucca-cli",
    ] {
        let sources = root.join(member).join("src");
        if !sources.is_dir() {
            continue;
        }
        walk(&sources, member, &mut found);
    }
    found.sort();
    found
}

fn walk(directory: &Path, member: &str, found: &mut Vec<(String, String, usize, String)>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, member, found);
            continue;
        }
        if path.extension().is_none_or(|extension| extension != "rs")
            || path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("_tests.rs"))
        {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        // Test modules assert on the English text; they add no messages.
        let code = text.split("#[cfg(test)]").next().unwrap_or(&text);
        for (line, message) in scan(code) {
            let file = path
                .strip_prefix(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .parent()
                        .unwrap_or(&path),
                )
                .unwrap_or(&path)
                .display()
                .to_string()
                .replace('\\', "/");
            found.push((member.to_owned(), file, line, message));
        }
    }
}

/// The message literals of one source file, with their line numbers.
fn scan(code: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = code.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i = (i + 2).min(chars.len());
            }
            '\'' => {
                // A character literal is `'x'`, `'\n'` or `'\\'`; a lifetime or a
                // lifetime-looking punctuation mark opens nothing at all.
                i += 1;
                i += usize::from(chars.get(i) == Some(&'\\')) + 1;
                i += usize::from(chars.get(i) == Some(&'\''));
            }
            '"' => {
                let (line, value, end) = literal(&chars, i);
                if let Some(callee) = callee(&chars, i) {
                    let into = matches!(chars.get(end), Some(&'.'))
                        && chars[end..].iter().take(7).collect::<String>() == ".into()";
                    if MESSAGE_CALLS.contains(&callee.as_str()) && (callee != "Error" || into) {
                        out.push((line, value));
                    }
                }
                i = end;
            }
            _ => i += 1,
        }
    }
    out
}

/// Reads one string literal, returning its line, unescaped text and end index.
fn literal(chars: &[char], start: usize) -> (usize, String, usize) {
    let line = chars[..start].iter().filter(|c| **c == '\n').count() + 1;
    let mut value = String::new();
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '"' => return (line, value, i + 1),
            '\\' if chars.get(i + 1) == Some(&'\n') => {
                // A continuation: the next line's indentation is not content.
                i += 2;
                while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
                    i += 1;
                }
            }
            '\\' if i + 1 < chars.len() => {
                let escaped = match chars[i + 1] {
                    'n' => '\n',
                    't' => '\t',
                    other => other,
                };
                value.push(escaped);
                i += 2;
            }
            other => {
                value.push(other);
                i += 1;
            }
        }
    }
    (line, value, i)
}

/// The call path whose argument list opens immediately before `quote`.
fn callee(chars: &[char], quote: usize) -> Option<String> {
    let mut i = quote;
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    if i == 0 || chars[i - 1] != '(' {
        return None;
    }
    let close = i - 1;
    let mut start = close;
    while start > 0
        && (chars[start - 1].is_alphanumeric()
            || chars[start - 1] == '_'
            || chars[start - 1] == ':')
    {
        start -= 1;
    }
    (start < close).then(|| chars[start..close].iter().collect())
}
