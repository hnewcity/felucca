//! The languages Felucca renders, and how a request picks one.

/// A language that has a translation catalog in this build.
///
/// The enum is the runtime handle for a language: catalog lookup and `Intl`
/// tags both key off it, so a `Language` is `Copy` and cheap to carry through a
/// request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    /// Simplified Chinese, the language the product shipped in first.
    Chinese,
    English,
    Japanese,
    Korean,
    French,
    Spanish,
    Russian,
}

impl Language {
    /// Every catalog this build ships, in the order the console lists them.
    pub const ALL: [Self; 7] = [
        Self::Chinese,
        Self::English,
        Self::Japanese,
        Self::Korean,
        Self::French,
        Self::Spanish,
        Self::Russian,
    ];

    /// The primary lower-case subtag: the catalog file name and API identifier.
    pub fn code(self) -> &'static str {
        match self {
            Self::Chinese => "zh",
            Self::English => "en",
            Self::Japanese => "ja",
            Self::Korean => "ko",
            Self::French => "fr",
            Self::Spanish => "es",
            Self::Russian => "ru",
        }
    }

    /// The BCP 47 tag browsers and the `Intl` APIs expect.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Chinese => "zh-CN",
            Self::English => "en-US",
            Self::Japanese => "ja-JP",
            Self::Korean => "ko-KR",
            Self::French => "fr-FR",
            Self::Spanish => "es-ES",
            Self::Russian => "ru-RU",
        }
    }

    /// The region subtag of [`Language::tag`], used to prefer `pt-BR` over `pt`.
    pub fn region(self) -> Option<&'static str> {
        self.tag().split('-').nth(1)
    }

    /// Resolves a tag such as `ja`, `ja-JP` or `zh-Hans-CN`.
    ///
    /// Region and script subtags only refine a match, so every spelling of a
    /// language resolves to the same variant.
    pub fn parse(tag: &str) -> Option<Self> {
        let primary = tag.split(['-', '_']).next()?;
        Self::ALL
            .into_iter()
            .find(|language| language.code().eq_ignore_ascii_case(primary))
    }
}

/// Reads a language from the environment.
///
/// `FELUCCA_LANG` outranks the shell's locale, so an operator can pin console
/// messages without changing the language of their own session. The POSIX
/// variables follow in their usual order, and `LANGUAGE` may be a colon
/// separated priority list; values such as `C` and `POSIX` resolve to nothing.
pub fn from_environment() -> Option<Language> {
    from_variables(|name| std::env::var(name).ok())
}

/// Reads a language from named variables, in the order that decides a message
/// language. `lookup` mirrors `std::env::var`, so the precedence stays testable
/// without mutating the process environment.
pub fn from_variables(lookup: impl Fn(&str) -> Option<String>) -> Option<Language> {
    let pinned = lookup("FELUCCA_LANG").as_deref().and_then(Language::parse);
    pinned.or_else(|| {
        ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"]
            .iter()
            .filter_map(|name| lookup(name))
            .find_map(|value| value.split(':').find_map(Language::parse))
    })
}

/// Picks the best language from an `Accept-Language` header value.
///
/// This is the part of RFC 4647 basic filtering Felucca needs: quality values
/// first, then the more specific range, then a region match ahead of a bare
/// language match. `None` means the client asked for nothing we ship, leaving
/// the default to the caller.
pub fn negotiate(accepted: &str, supported: &[Language]) -> Option<Language> {
    let mut best: Option<(f32, u8, Language)> = None;
    for range in accepted.split(',') {
        let (tag, quality) = split_range(range);
        if quality <= 0.0 || tag == "*" {
            continue;
        }
        let Some(language) = Language::parse(tag) else {
            continue;
        };
        let Some(supported) = supported.contains(&language).then_some(language) else {
            continue;
        };
        let region = region_of(tag);
        let specificity = u8::from(region.is_some_and(|region| supported.region() == Some(region)));
        if best.is_none_or(|(best_quality, best_specificity, _)| {
            quality > best_quality || (quality == best_quality && specificity > best_specificity)
        }) {
            best = Some((quality, specificity, supported));
        }
    }
    best.map(|(_, _, language)| language)
}

/// Splits one header range into its tag and quality value.
fn split_range(range: &str) -> (&str, f32) {
    let mut parts = range.split(';');
    let tag = parts.next().unwrap_or_default().trim();
    let quality = parts
        .filter_map(|parameter| {
            let (name, value) = parameter.trim().split_once('=')?;
            name.eq_ignore_ascii_case("q")
                .then(|| value.trim().parse().ok())?
        })
        .next()
        .unwrap_or(1.0);
    (tag, quality)
}

/// The two-letter region subtag of a tag, if it carries one.
fn region_of(tag: &str) -> Option<&str> {
    let mut parts = tag.split('-');
    parts.next()?;
    parts
        .next()
        .filter(|part| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_alphabetic()))
}

/// Resolves the language for headers, preferring an explicit client choice.
///
/// `X-Felucca-Locale` lets a caller pin a language independently of what their
/// browser happens to send — the console sends the language its operator
/// picked, so an English browser can still read Japanese messages.
pub fn from_headers(explicit: Option<&str>, accepted: Option<&str>) -> Option<Language> {
    explicit
        .and_then(Language::parse)
        .or_else(|| accepted.and_then(|accepted| negotiate(accepted, &Language::ALL)))
}
