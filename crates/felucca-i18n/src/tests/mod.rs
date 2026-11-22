mod sources;

use super::catalog::entries;
use super::*;
use crate::Message;
use std::collections::BTreeSet;

#[test]
fn parses_every_spelling_of_a_language() {
    for (tag, expected) in [
        ("ja", Language::Japanese),
        ("JA", Language::Japanese),
        ("ja-JP", Language::Japanese),
        ("zh-Hans-CN", Language::Chinese),
        ("zh_CN.UTF-8", Language::Chinese),
        ("ru", Language::Russian),
    ] {
        assert_eq!(Language::parse(tag), Some(expected), "{tag}");
    }
    for tag in ["", "*", "C", "POSIX", "de", "xx-YY"] {
        assert_eq!(Language::parse(tag), None, "{tag}");
    }
    assert_eq!(Language::French.tag(), "fr-FR");
    assert_eq!(Language::French.region(), Some("FR"));
}

#[test]
fn orders_preferences_by_quality_then_specificity() {
    let supported = Language::ALL;
    assert_eq!(
        negotiate("ja, en;q=0.5", &supported),
        Some(Language::Japanese)
    );
    assert_eq!(
        negotiate("en;q=0.5, ja", &supported),
        Some(Language::Japanese)
    );
    // A region match beats a bare language match at the same quality.
    assert_eq!(
        negotiate("ja;q=0.9, es-ES;q=0.9", &supported),
        Some(Language::Spanish)
    );
    // Quality wins over specificity.
    assert_eq!(
        negotiate("es-ES;q=0.2, ja;q=0.8", &supported),
        Some(Language::Japanese)
    );
    // A refused language is skipped, not treated as the best match.
    assert_eq!(negotiate("ja;q=0, ko", &supported), Some(Language::Korean));
    // `*` asks for anything we ship; the caller applies its own default.
    assert_eq!(negotiate("*", &supported), None);
    assert_eq!(negotiate("de", &supported), None);
    assert_eq!(
        negotiate("de, fr-CH", &[Language::French, Language::English]),
        Some(Language::French),
        "a region of a supported language still matches that language"
    );
}

#[test]
fn reads_locale_from_variables_in_precedence_order() {
    // A borrowed table stands in for the environment, so precedence is checked
    // without mutating the process.
    fn lookup<'a>(
        values: &'a [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Option<String> + 'a {
        move |name: &str| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }
    let pinned = [("FELUCCA_LANG", "fr"), ("LANG", "ja_JP.UTF-8")];
    assert_eq!(
        from_variables(lookup(&pinned)),
        Some(Language::French),
        "a pinned language outranks the shell locale"
    );
    let posix = [
        ("LC_ALL", "ru_RU.UTF-8"),
        ("LC_MESSAGES", "ko"),
        ("LANG", "ja"),
    ];
    assert_eq!(from_variables(lookup(&posix)), Some(Language::Russian));
    let listed = [("LANGUAGE", "es:ja"), ("LANG", "ko")];
    assert_eq!(
        from_variables(lookup(&listed)),
        Some(Language::Spanish),
        "LANGUAGE is a priority list"
    );
    let unsupported = [("LC_ALL", "C"), ("LANG", "POSIX")];
    assert_eq!(from_variables(lookup(&unsupported)), None);
    assert_eq!(from_variables(|_| None), None);
}

#[test]
fn prefers_an_explicit_header_over_browser_preferences() {
    assert_eq!(
        from_headers(Some("ko"), Some("ja, en;q=0.9")),
        Some(Language::Korean)
    );
    assert_eq!(
        from_headers(Some("de"), Some("es")),
        Some(Language::Spanish),
        "an unusable explicit value falls back to the header"
    );
    assert_eq!(from_headers(None, Some("ru")), Some(Language::Russian));
    assert_eq!(from_headers(None, None), None);
}

#[test]
fn renders_english_until_a_translation_exists() {
    // A message no catalog carries is English in every language, never empty and
    // never a panic: a new string is safe to ship before anyone translates it.
    let message = Message::text("a message no catalog carries");
    for language in Language::ALL {
        assert_eq!(
            message.render(language),
            "a message no catalog carries",
            "{language:?}"
        );
    }
    // English itself is a catalog, so a message it carries renders as itself.
    let translated = Message::text("permission denied");
    assert_eq!(translated.render(Language::English), "permission denied");
}

#[test]
fn interpolates_named_values() {
    let message = Message::with(
        "a bind host must be an IP address, not {host}",
        [Value::new("host", "\"localhost\"")],
    );
    assert_eq!(
        message.to_string(),
        "a bind host must be an IP address, not \"localhost\""
    );
    assert_eq!(
        message.msgid(),
        "a bind host must be an IP address, not {host}"
    );
    let unknown = Message::text("use {missing} sparingly");
    assert_eq!(unknown.to_string(), "use {missing} sparingly");
    let braces = Message::text("allowed_ports = [8080, {{ from = 9000 }}]");
    assert!(braces.to_string().contains("from = 9000"));
}

#[test]
fn catalogs_agree_on_keys_and_placeholders() {
    let reference = entries(Language::English);
    assert!(!reference.is_empty(), "the English catalog is the key list");
    fn placeholders(text: &str) -> Vec<&str> {
        let mut names: Vec<&str> = text
            .split('{')
            .skip(1)
            .filter_map(|part| part.split('}').next())
            .filter(|name| !name.is_empty())
            .collect();
        names.sort_unstable();
        names
    }
    for language in Language::ALL {
        let catalog = entries(language);
        if language == Language::English {
            continue;
        }
        for key in reference.keys() {
            assert!(catalog.contains_key(key), "{language:?} is missing {key:?}");
        }
        for (key, value) in catalog {
            let Some(english) = reference.get(key) else {
                panic!("{language:?} has a key the source no longer uses: {key:?}");
            };
            assert!(
                !value.is_empty(),
                "{language:?} has an empty value for {key:?}"
            );
            assert_eq!(
                placeholders(value),
                placeholders(english),
                "{language:?} changed the placeholders of {key:?}"
            );
        }
    }
}

#[test]
fn sources_and_catalog_agree() {
    let catalog = entries(Language::English);
    let found: BTreeSet<String> = sources::messages()
        .into_iter()
        .map(|(_, _, _, message)| message)
        .collect();
    let registered: BTreeSet<String> = catalog.keys().cloned().collect();
    let unregistered = found.difference(&registered);
    let stale = registered.difference(&found);
    assert!(
        unregistered.clone().chain(stale.clone()).next().is_none(),
        "the English catalog and the sources disagree: not in the catalog {:?}, no longer used {:?}",
        unregistered.collect::<Vec<_>>(),
        stale.collect::<Vec<_>>(),
    );
    assert!(found.len() > 100, "the scan found {} messages", found.len());
}
