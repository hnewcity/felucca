//! A message: an English template plus the values it interpolates.

use super::{Language, catalog};
use std::{borrow::Cow, fmt};

/// One interpolated value.
///
/// Names appear in the template as `{name}`, so a translation may reorder,
/// duplicate or drop them without changing the call site that built the
/// message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    name: &'static str,
    text: Text,
}

impl Value {
    /// A value shown as it is.
    pub fn new(name: &'static str, text: impl fmt::Display) -> Self {
        Self {
            name,
            text: Text::Plain(text.to_string()),
        }
    }

    /// A value shown the way `{:?}` would show it.
    ///
    /// Several messages quote what the operator typed, which needs debug
    /// formatting to stay unambiguous about where the text begins and ends.
    pub fn debug(name: &'static str, value: impl fmt::Debug) -> Self {
        Self {
            name,
            text: Text::Plain(format!("{value:?}")),
        }
    }

    /// A value that is itself a message, rendered in the same language.
    ///
    /// Configuration errors wrap other configuration errors; without this the
    /// wrapper would translate but its cause would not.
    pub fn nested(name: &'static str, message: Message) -> Self {
        Self {
            name,
            text: Text::Nested(message),
        }
    }
}

/// What a value contributes to the rendered text.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Text {
    Plain(String),
    Nested(Message),
}

impl Text {
    fn render(&self, language: Language) -> String {
        match self {
            Self::Plain(text) => text.clone(),
            Self::Nested(message) => message.render(language),
        }
    }
}

/// A localizable message.
///
/// `Display` renders the English source, which keeps logs and existing
/// assertions on error text unchanged; [`Message::render`] produces the text a
/// caller's locale should see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    template: Cow<'static, str>,
    values: Vec<Value>,
}

impl Message {
    /// A message with nothing to interpolate, the common case.
    pub const fn text(template: &'static str) -> Self {
        Self {
            template: Cow::Borrowed(template),
            values: Vec::new(),
        }
    }

    /// A message whose text carries values named in the template.
    pub fn with(
        template: impl Into<Cow<'static, str>>,
        values: impl IntoIterator<Item = Value>,
    ) -> Self {
        Self {
            template: template.into(),
            values: values.into_iter().collect(),
        }
    }

    /// The catalog key, or the already formatted text when the message was
    /// built from a runtime string.
    pub fn msgid(&self) -> &str {
        &self.template
    }

    /// The text this language should show, falling back to English.
    pub fn render(&self, language: Language) -> String {
        match catalog::translate(language, &self.template) {
            Some(translated) => interpolate(translated, &self.values, language),
            None => self.english(),
        }
    }

    /// The English source with values interpolated.
    pub fn english(&self) -> String {
        interpolate(&self.template, &self.values, Language::English)
    }
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.english())
    }
}

impl From<&str> for Message {
    /// A message built from a string literal, the usual case, or from text this
    /// process assembled at runtime.
    ///
    /// Only the literal becomes a catalog key; runtime text carries no key and
    /// stays English.
    fn from(template: &str) -> Self {
        Self::with(template.to_owned(), [])
    }
}
impl From<String> for Message {
    fn from(text: String) -> Self {
        Self::with(text, [])
    }
}

/// Substitutes `{name}` placeholders, leaving anything unmatched untouched.
///
/// An unmatched placeholder is visible rather than empty, so a template and its
/// translation that disagree still produce something an operator can report.
fn interpolate(template: &str, values: &[Value], language: Language) -> String {
    if !template.contains('{') {
        return template.to_owned();
    }
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            break;
        };
        let name = &after[..close];
        match values.iter().find(|value| value.name == name) {
            Some(value) => out.push_str(&value.text.render(language)),
            None => {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}
