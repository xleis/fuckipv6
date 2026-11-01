//! Shared localization contract for native, browser, and future Android surfaces.
//!
//! The JSON catalog in `assets/i18n.json` is the single source of visible copy.
//! Rust exposes typed locale/preference handling and deterministic formatting;
//! the browser receives the exact same catalog at runtime.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::LazyLock;
use std::time::Duration;

use chrono::{DateTime, Datelike, FixedOffset, Timelike};
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

/// A concrete language supported by the first product release.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum Locale {
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[default]
    #[serde(rename = "en")]
    En,
}

impl Locale {
    pub const ALL: [Self; 2] = [Self::ZhCn, Self::En];

    pub const fn language_tag(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN",
            Self::En => "en",
        }
    }

    /// Match browser/OS language tags conservatively. Chinese variants use the
    /// Simplified catalog in the first release; every non-Chinese tag uses the
    /// English fallback instead of silently showing mixed-language copy.
    pub fn from_tag(tag: &str) -> Self {
        let normalized = tag.trim().replace('_', "-").to_ascii_lowercase();
        if normalized == "zh" || normalized.starts_with("zh-") {
            Self::ZhCn
        } else {
            Self::En
        }
    }
}

impl fmt::Display for Locale {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.language_tag())
    }
}

/// A node owner's persisted language choice. `System` is resolved each time the
/// surface starts, while explicit choices remain stable across environments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LanguagePreference {
    #[default]
    #[serde(rename = "system")]
    System,
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en")]
    En,
}

impl LanguagePreference {
    pub const ALL: [Self; 3] = [Self::System, Self::ZhCn, Self::En];

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::System => "lang.system",
            Self::ZhCn => "lang.zh_cn",
            Self::En => "lang.en",
        }
    }

    pub fn resolve(self, system_locale: Locale) -> Locale {
        match self {
            Self::System => system_locale,
            Self::ZhCn => Locale::ZhCn,
            Self::En => Locale::En,
        }
    }

    pub const fn storage_value(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::ZhCn => "zh-CN",
            Self::En => "en",
        }
    }
}

impl fmt::Display for LanguagePreference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.storage_value())
    }
}

impl FromStr for LanguagePreference {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().replace('_', "-").to_ascii_lowercase().as_str() {
            "system" | "auto" | "" => Ok(Self::System),
            "zh-cn" | "zh" | "zh-hans" => Ok(Self::ZhCn),
            "en" | "en-us" | "en-gb" => Ok(Self::En),
            _ => Err("unsupported language preference"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Catalog {
    schema_version: u32,
    default_locale: Locale,
    messages: BTreeMap<Locale, BTreeMap<String, String>>,
}

static CATALOG_JSON: &str = include_str!("../assets/i18n.json");
static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    let catalog: Catalog =
        serde_json::from_str(CATALOG_JSON).expect("embedded i18n catalog must be valid JSON");
    assert_eq!(catalog.schema_version, 1, "unsupported i18n catalog schema");
    catalog
});

/// Return the raw catalog served to browser and future platform adapters.
pub const fn catalog_json() -> &'static str {
    CATALOG_JSON
}

pub fn default_locale() -> Locale {
    CATALOG.default_locale
}

/// Translate a stable key. Unknown keys fail visibly by returning the key rather
/// than falling back to another language or a natural-language business value.
pub fn t(locale: Locale, key: &str) -> &str {
    CATALOG
        .messages
        .get(&locale)
        .and_then(|messages| messages.get(key))
        .map(String::as_str)
        .unwrap_or(key)
}

pub fn has_key(locale: Locale, key: &str) -> bool {
    CATALOG
        .messages
        .get(&locale)
        .is_some_and(|messages| messages.contains_key(key))
}

pub fn message_keys(locale: Locale) -> BTreeSet<&'static str> {
    CATALOG
        .messages
        .get(&locale)
        .into_iter()
        .flat_map(|messages| messages.keys().map(String::as_str))
        .collect()
}

/// Replace `{name}` placeholders with structured values. Missing values remain
/// visible so contract regressions are caught instead of silently dropping text.
pub fn interpolate(template: &str, values: &[(&str, &str)]) -> String {
    let mut text = template.to_string();
    for (name, value) in values {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

pub fn tr(locale: Locale, key: &str, values: &[(&str, &str)]) -> String {
    interpolate(t(locale, key), values)
}

/// Resolve the operating-system language. Environment variables are checked
/// first because they are the deterministic mechanism used by service managers
/// and tests; `sys-locale` provides the desktop fallback.
pub fn detect_system_locale() -> Locale {
    for name in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(name) {
            if !value.trim().is_empty() {
                return Locale::from_tag(&value);
            }
        }
    }
    Locale::from_tag(&sys_locale::get_locale().unwrap_or_else(|| "en".into()))
}

pub fn resolve_language(preference: LanguagePreference) -> Locale {
    preference.resolve(detect_system_locale())
}

/// Format an integer with locale-appropriate grouping. Chinese (China) and
/// English both use comma grouping and dot decimals in the first release.
pub fn format_integer(locale: Locale, value: i64) -> String {
    let (separator, _) = number_symbols(locale);
    let sign = if value < 0 { "-" } else { "" };
    let digits = value.unsigned_abs().to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(separator);
        }
        grouped.push(character);
    }
    format!("{sign}{grouped}")
}

pub fn format_decimal(locale: Locale, value: f64, precision: usize) -> String {
    if !value.is_finite() {
        return t(locale, "common.not_available").into();
    }
    let (group_separator, decimal_separator) = number_symbols(locale);
    let formatted = format!("{value:.precision$}");
    let (integer, fraction) = formatted.split_once('.').unwrap_or((&formatted, ""));
    let integer = format_integer(locale, integer.parse::<i64>().unwrap_or_default())
        .replace(',', &group_separator.to_string());
    if precision == 0 {
        integer
    } else {
        format!("{integer}{decimal_separator}{fraction}")
    }
}

const fn number_symbols(_locale: Locale) -> (char, char) {
    (',', '.')
}

/// Format a timestamp with an explicit timezone. Address-like values remain
/// locale independent; timestamps never rely on the host's implicit timezone.
pub fn format_timestamp(locale: Locale, timestamp: &DateTime<FixedOffset>) -> String {
    let timezone = explicit_timezone_label(timestamp.offset().local_minus_utc());
    match locale {
        Locale::ZhCn => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {timezone}",
            timestamp.year(),
            timestamp.month(),
            timestamp.day(),
            timestamp.hour(),
            timestamp.minute(),
            timestamp.second()
        ),
        Locale::En => {
            let hour = timestamp.hour();
            let twelve_hour = match hour % 12 {
                0 => 12,
                value => value,
            };
            let period = if hour < 12 { "AM" } else { "PM" };
            format!(
                "{} {}, {:04}, {}:{:02}:{:02} {period} {timezone}",
                english_month(timestamp.month()),
                timestamp.day(),
                timestamp.year(),
                twelve_hour,
                timestamp.minute(),
                timestamp.second()
            )
        }
    }
}

fn explicit_timezone_label(offset_seconds: i32) -> String {
    if offset_seconds == 0 {
        return "UTC+00:00".into();
    }
    let sign = if offset_seconds < 0 { '-' } else { '+' };
    let absolute = offset_seconds.unsigned_abs();
    format!(
        "UTC{sign}{:02}:{:02}",
        absolute / 3600,
        (absolute % 3600) / 60
    )
}

const fn english_month(month: u32) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        _ => "Dec",
    }
}

pub fn format_bytes(locale: Locale, bytes: u64) -> String {
    const UNITS: [(&str, u64); 4] = [
        ("B", 1),
        ("KiB", 1 << 10),
        ("MiB", 1 << 20),
        ("GiB", 1 << 30),
    ];
    let (unit, divisor) = UNITS
        .iter()
        .rev()
        .find(|(_, divisor)| bytes >= *divisor)
        .copied()
        .unwrap_or(("B", 1));
    if divisor == 1 {
        format!("{} {unit}", format_integer(locale, bytes as i64))
    } else {
        format!(
            "{} {unit}",
            format_decimal(locale, bytes as f64 / divisor as f64, 1)
        )
    }
}

pub fn format_duration(locale: Locale, duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        return match locale {
            Locale::ZhCn => format!("{seconds} 秒"),
            Locale::En => format!("{seconds}s"),
        };
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        let remainder = seconds % 60;
        return match locale {
            Locale::ZhCn => format!("{minutes} 分 {remainder} 秒"),
            Locale::En => format!("{minutes}m {remainder}s"),
        };
    }
    let hours = minutes / 60;
    let remainder = minutes % 60;
    match locale {
        Locale::ZhCn => format!("{hours} 小时 {remainder} 分"),
        Locale::En => format!("{hours}h {remainder}m"),
    }
}

/// Format an endpoint without translating the address or protocol. IPv6 is
/// bracketed when a port is present so the result is unambiguous.
pub fn format_socket_address(address: IpAddr, port: u16) -> String {
    match address {
        IpAddr::V4(address) => format!("{address}:{port}"),
        IpAddr::V6(address) => format!("[{address}]:{port}"),
    }
}

fn pseudo_expand_letters(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            'a' => 'á',
            'e' => 'é',
            'i' => 'í',
            'o' => 'ó',
            'u' => 'ú',
            'A' => 'Á',
            'E' => 'É',
            'I' => 'Í',
            'O' => 'Ó',
            'U' => 'Ú',
            character => character,
        })
        .collect()
}

fn ascii_placeholder_end(text: &str, start: usize) -> Option<usize> {
    let tail = text.get(start + 1..)?;
    let relative_end = tail.find('}')?;
    let name = &tail[..relative_end];
    (!name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')))
    .then_some(start + 1 + relative_end)
}

/// Produce an intentionally expanded, bracketed pseudo locale. It exposes
/// clipping and hard-coded-width regressions without changing key semantics.
/// Structured `{name}` interpolation tokens remain byte-for-byte unchanged.
pub fn pseudo_localize(text: &str) -> String {
    let mut expanded = String::with_capacity(text.len() + 2);
    let mut cursor = 0;
    while let Some(relative_start) = text[cursor..].find('{') {
        let start = cursor + relative_start;
        expanded.push_str(&pseudo_expand_letters(&text[cursor..start]));
        if let Some(end) = ascii_placeholder_end(text, start) {
            expanded.push_str(&text[start..=end]);
            cursor = end + 1;
        } else {
            let character_end = start + text[start..].chars().next().map_or(0, char::len_utf8);
            expanded.push_str(&pseudo_expand_letters(&text[start..character_end]));
            cursor = character_end;
        }
    }
    expanded.push_str(&pseudo_expand_letters(&text[cursor..]));
    format!("⟦{expanded}⟧")
}

/// Truncate by Unicode grapheme clusters, never bytes or scalar values. The
/// ellipsis is included in `max_chars`; storing the full value remains the
/// caller's job.
pub fn truncate_unicode(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if text.graphemes(true).count() <= max_chars {
        return text.to_string();
    }
    if max_chars == 1 {
        return "…".into();
    }
    let mut truncated: String = text.graphemes(true).take(max_chars - 1).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn placeholders(text: &str) -> BTreeSet<&str> {
        let mut found = BTreeSet::new();
        let mut cursor = 0;
        while let Some(relative_start) = text[cursor..].find('{') {
            let start = cursor + relative_start;
            let Some(end) = ascii_placeholder_end(text, start) else {
                cursor = start + 1;
                continue;
            };
            found.insert(&text[start..=end]);
            cursor = end + 1;
        }
        found
    }

    #[test]
    fn catalogs_have_identical_non_empty_keys() {
        let zh = message_keys(Locale::ZhCn);
        let en = message_keys(Locale::En);
        assert_eq!(zh, en);
        assert!(!zh.is_empty());
        for key in zh {
            assert!(
                !t(Locale::ZhCn, key).trim().is_empty(),
                "empty zh key {key}"
            );
            assert!(!t(Locale::En, key).trim().is_empty(), "empty en key {key}");
        }
    }

    #[test]
    fn pseudo_locale_is_complete_and_preserves_every_placeholder() {
        for locale in Locale::ALL {
            for key in message_keys(locale) {
                let original = t(locale, key);
                let pseudo = pseudo_localize(original);
                assert!(
                    pseudo.starts_with('⟦') && pseudo.ends_with('⟧'),
                    "pseudo locale markers missing for {locale}/{key}"
                );
                assert!(
                    !pseudo.trim_matches(&['⟦', '⟧'][..]).trim().is_empty(),
                    "pseudo locale is empty for {locale}/{key}"
                );
                assert_eq!(
                    placeholders(original),
                    placeholders(&pseudo),
                    "placeholder changed for {locale}/{key}"
                );
                assert!(
                    !pseudo.contains('\u{FFFD}'),
                    "invalid Unicode in pseudo locale for {locale}/{key}"
                );
            }
        }
    }

    #[test]
    fn language_preferences_follow_system_or_manual_choice() {
        assert_eq!(
            LanguagePreference::System.resolve(Locale::ZhCn),
            Locale::ZhCn
        );
        assert_eq!(LanguagePreference::ZhCn.resolve(Locale::En), Locale::ZhCn);
        assert_eq!(LanguagePreference::En.resolve(Locale::ZhCn), Locale::En);
        assert_eq!("system".parse(), Ok(LanguagePreference::System));
        assert_eq!("zh-Hans".parse(), Ok(LanguagePreference::ZhCn));
        assert_eq!("en-US".parse(), Ok(LanguagePreference::En));
        assert!("fr".parse::<LanguagePreference>().is_err());
    }

    #[test]
    fn locale_matching_uses_simplified_chinese_or_english_fallback() {
        assert_eq!(Locale::from_tag("zh-CN"), Locale::ZhCn);
        assert_eq!(Locale::from_tag("zh_Hans_CN"), Locale::ZhCn);
        assert_eq!(Locale::from_tag("en-US"), Locale::En);
        assert_eq!(Locale::from_tag("fr-FR"), Locale::En);
        assert_eq!(Locale::from_tag("C"), Locale::En);
    }

    #[test]
    fn interpolation_is_structured_and_missing_parameters_stay_visible() {
        assert_eq!(
            interpolate(
                "Network {network}: {count}",
                &[("network", "home"), ("count", "2")]
            ),
            "Network home: 2"
        );
        assert_eq!(interpolate("{missing}", &[]), "{missing}");
        assert_eq!(tr(Locale::ZhCn, "unknown.key", &[]), "unknown.key");
    }

    #[test]
    fn numbers_time_zones_units_and_addresses_are_deterministic() {
        assert_eq!(format_integer(Locale::En, 1_234_567), "1,234,567");
        assert_eq!(format_integer(Locale::En, -1_234_567), "-1,234,567");
        assert_eq!(format_integer(Locale::En, 0), "0");
        assert_eq!(format_decimal(Locale::En, 12.345, 1), "12.3");
        assert_eq!(format_decimal(Locale::ZhCn, -12.345, 2), "-12.35");
        assert_eq!(
            format_decimal(Locale::En, f64::NAN, 1),
            t(Locale::En, "common.not_available")
        );
        assert_eq!(format_bytes(Locale::En, 0), "0 B");
        assert_eq!(format_bytes(Locale::En, 1_048_576), "1.0 MiB");
        assert_eq!(
            format_duration(Locale::ZhCn, Duration::from_secs(125)),
            "2 分 5 秒"
        );
        assert_eq!(
            format_duration(Locale::En, Duration::from_secs(3605)),
            "1h 0m"
        );
        assert_eq!(
            format_socket_address("192.0.2.1".parse().unwrap(), 8080),
            "192.0.2.1:8080"
        );
        assert_eq!(
            format_socket_address("2001:db8::1".parse().unwrap(), 443),
            "[2001:db8::1]:443"
        );

        let offset = FixedOffset::east_opt(8 * 3600).unwrap();
        let timestamp = offset.with_ymd_and_hms(2026, 11, 1, 17, 3, 4).unwrap();
        assert_eq!(
            format_timestamp(Locale::ZhCn, &timestamp),
            "2026-11-01 17:03:04 UTC+08:00"
        );
        assert_eq!(
            format_timestamp(Locale::En, &timestamp),
            "Nov 1, 2026, 5:03:04 PM UTC+08:00"
        );
    }

    #[test]
    fn pseudolocale_expands_without_changing_interpolation_tokens() {
        let pseudo = pseudo_localize("Network {network}");
        assert!(pseudo.starts_with('⟦'));
        assert!(pseudo.ends_with('⟧'));
        assert!(pseudo.chars().count() > "Network {network}".chars().count());
        assert!(pseudo.contains("{network}"));
        assert!(!pseudo.contains("{nétwórk}"));
    }

    #[test]
    fn unicode_truncation_is_grapheme_safe_and_honors_boundaries() {
        let combining = "e\u{301}xy";
        let family = "👨‍👩‍👧‍👦";

        assert_eq!(truncate_unicode("网络节点", 3), "网络…");
        assert_eq!(truncate_unicode(combining, 2), "e\u{301}…");
        assert_eq!(truncate_unicode(combining, 3), combining);
        assert_eq!(truncate_unicode(family, 1), family);
        assert_eq!(truncate_unicode(&format!("{family}x"), 1), "…");
        assert_eq!(
            truncate_unicode(&format!("{family}xy"), 2),
            format!("{family}…")
        );
        assert_eq!(
            truncate_unicode(&format!("{family}x"), 2),
            format!("{family}x")
        );
        assert_eq!(truncate_unicode("short", 10), "short");
        assert_eq!(truncate_unicode("anything", 0), "");
    }
}
