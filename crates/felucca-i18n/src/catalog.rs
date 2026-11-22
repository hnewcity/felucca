//! Embedded message catalogs.
//!
//! English is the message identity. `en.json` holds that identity — the English
//! source text of every localizable string in the workspace, extracted from the
//! call sites and kept honest by `tests::sources_and_catalog_agree` — and every
//! other language carries exactly the same keys.

use super::Language;
use std::{collections::HashMap, sync::LazyLock};

type Catalog = HashMap<String, String>;

macro_rules! catalog {
    ($code:literal) => {
        LazyLock::new(|| parse(include_str!(concat!("../catalogs/", $code, ".json"))))
    };
}

static ENGLISH: LazyLock<Catalog> = catalog!("en");
static CHINESE: LazyLock<Catalog> = catalog!("zh");
static JAPANESE: LazyLock<Catalog> = catalog!("ja");
static KOREAN: LazyLock<Catalog> = catalog!("ko");
static FRENCH: LazyLock<Catalog> = catalog!("fr");
static SPANISH: LazyLock<Catalog> = catalog!("es");
static RUSSIAN: LazyLock<Catalog> = catalog!("ru");

fn parse(source: &'static str) -> Catalog {
    serde_json::from_str(source).expect("felucca ships valid message catalogs")
}

fn catalog_for(language: Language) -> &'static Catalog {
    match language {
        Language::Chinese => &CHINESE,
        Language::English => &ENGLISH,
        Language::Japanese => &JAPANESE,
        Language::Korean => &KOREAN,
        Language::French => &FRENCH,
        Language::Spanish => &SPANISH,
        Language::Russian => &RUSSIAN,
    }
}

/// The translation of one English message, or `None` when it is untranslated.
pub fn translate(language: Language, msgid: &str) -> Option<&'static str> {
    catalog_for(language).get(msgid).map(String::as_str)
}

/// Every entry of one catalog, for completeness checks.
#[cfg(test)]
pub(crate) fn entries(language: Language) -> &'static Catalog {
    catalog_for(language)
}
