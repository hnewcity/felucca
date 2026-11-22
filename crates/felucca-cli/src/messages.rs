//! The language this process writes its messages in.

use felucca_i18n::{Language, Message};
use std::sync::OnceLock;

static LANGUAGE: OnceLock<Language> = OnceLock::new();

/// Records the language for everything this process prints, once.
///
/// A command line is scripted, so the language follows an explicit choice only:
/// `--lang`, then `FELUCCA_LANG`. Reading the shell's locale would make the same
/// command print different text on two machines, which is exactly what a script
/// cannot cope with. The console follows the browser instead.
pub fn adopt(explicit: Option<&str>) -> Language {
    let language = explicit
        .and_then(Language::parse)
        .or_else(|| {
            std::env::var("FELUCCA_LANG")
                .ok()
                .as_deref()
                .and_then(Language::parse)
        })
        .unwrap_or(Language::English);
    let _ = LANGUAGE.set(language);
    language
}

/// The language this process writes in, before and after [`adopt`].
pub fn language() -> Language {
    LANGUAGE.get().copied().unwrap_or(Language::English)
}

/// One message rendered in that language.
pub fn say(message: impl Into<Message>) -> String {
    message.into().render(language())
}

/// Prints a failure the way this process writes everything else.
///
/// `anyhow` renders each link of a chain with `Display`, and a configuration
/// error's `Display` is deliberately its English source text — a log wants the
/// message exactly as the code wrote it. A terminal does not, so the chain is
/// re-rendered here, translating the links that carry a catalog key and leaving
/// the rest (an operating system error, say) as the platform words them.
pub fn report(error: &anyhow::Error) {
    let links: Vec<String> = error.chain().map(link_text).collect();
    let Some((first, causes)) = links.split_first() else {
        return;
    };
    eprintln!("Error: {first}");
    if !causes.is_empty() {
        eprintln!("\nCaused by:");
        for (index, link) in causes.iter().enumerate() {
            eprintln!("    {index}: {link}");
        }
    }
}

/// One link of a failure chain, in this process's language where it can be.
fn link_text(link: &(dyn std::error::Error + 'static)) -> String {
    match link.downcast_ref::<felucca_core::error::ConfigError>() {
        Some(config) => config.message().render(language()),
        None => link.to_string(),
    }
}
