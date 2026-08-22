//! `Intl`, over ICU4X and the platform's own time-zone database (issue #237).
//!
//! Every other absence in Blitsen's subset has a reduced-capability fallback an
//! application can write for itself. `Intl` does not: the placement of a
//! currency symbol, the minor units a currency has, and the offset a named IANA
//! zone was at on a particular instant are data, not algorithms, and code that
//! guesses at them is wrong rather than approximate. So this is implemented
//! rather than documented away, and the data comes from CLDR through ICU4X and
//! from the platform's `tzdb` through `jiff` — the two sources a browser's own
//! ICU build reads.
//!
//! **What crosses the boundary is a handle, not a formatter.** Constructing an
//! `Intl.NumberFormat` builds native state that would be expensive to rebuild
//! per call, and applications construct them in render paths — inside a table
//! cell, inside a chart tick callback — where the same options recur thousands
//! of times. So a formatter is keyed by its resolved options and shared: the
//! second `new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" })`
//! is a map lookup rather than a second copy of the CLDR data behind it. That
//! also settles the lifetime question the engine boundary would otherwise raise
//! — nothing here has to be freed when a JavaScript object is collected,
//! because nothing here belongs to one.
//!
//! **Only what is honoured is reported.** `resolvedOptions()` is answered from
//! the values this module actually used, not from what the application asked
//! for, because an option echoed back unimplemented is the failure mode the
//! whole `doctor` surface exists to prevent.

mod datetime;
mod number;
mod text;

use std::cell::RefCell;
use std::collections::HashMap;

use blitsen_js::JsError;
use icu_locale_core::Locale;
use serde_json::{Value, json};

/// What the bootstrap asks for, and what it gets back a handle to.
enum Formatter {
    Number(number::NumberFormat),
    // Boxed because a date formatter carries its pattern data inline and is an
    // order of magnitude larger than the rest, which would otherwise be the
    // size of every entry in the table.
    DateTime(Box<datetime::DateTimeFormat>),
    RelativeTime(datetime::RelativeTimeFormat),
    Plural(text::PluralRules),
    Collator(text::Collator),
    List(text::ListFormat),
}

/// The `Intl` implementation owned by one JavaScript context.
#[derive(Default)]
pub(super) struct IntlHost {
    /// Formatters by the key their options canonicalise to, so the same options
    /// asked for twice are one formatter.
    handles: RefCell<HashMap<String, usize>>,
    /// Each formatter beside the options it honoured, which is what
    /// `resolvedOptions()` is answered from.
    formatters: RefCell<Vec<(Formatter, Value)>>,
}

/// Reads a string option, or `None` when it is absent or null.
fn option<'a>(options: &'a Value, name: &str) -> Option<&'a str> {
    options.get(name).and_then(Value::as_str)
}

/// Reads an option that is a small count.
fn count(options: &Value, name: &str) -> Option<u8> {
    options
        .get(name)
        .and_then(Value::as_u64)
        .map(|value| value.min(u64::from(u8::MAX)) as u8)
}

/// Reads a boolean option.
fn flag(options: &Value, name: &str) -> Option<bool> {
    options.get(name).and_then(Value::as_bool)
}

/// Parses the locale the bootstrap resolved, falling back to the host's.
///
/// The bootstrap has already reduced the requested list to one tag; what can
/// still arrive here is a tag no parser accepts, which is the application's own
/// string and is refused with the name in the message rather than silently
/// becoming English.
fn locale(options: &Value) -> Result<Locale, JsError> {
    let requested = option(options, "locale").unwrap_or_default();
    if requested.is_empty() {
        return Ok(default_locale());
    }
    requested
        .parse::<Locale>()
        .map_err(|error| JsError::new(format!("invalid locale {requested}: {error}")))
}

/// The locale the host is configured for, or `en-US` where it says nothing.
///
/// Read once: an application that changes its process environment mid-run is
/// not something `Intl` is required to notice, and re-reading per formatter
/// would make the same call answer differently over a session.
pub(crate) fn default_locale() -> Locale {
    thread_local! {
        static DEFAULT: Locale = sys_locale::get_locale()
            .and_then(|tag| tag.parse::<Locale>().ok())
            .unwrap_or(icu_locale_core::locale!("en-US"));
    }
    DEFAULT.with(Clone::clone)
}

/// The IANA zone the host is in, or `UTC` where the platform will not say.
pub(crate) fn default_time_zone() -> String {
    thread_local! {
        static DEFAULT: String = jiff::tz::TimeZone::system()
            .iana_name()
            .unwrap_or("UTC")
            .to_owned();
    }
    DEFAULT.with(Clone::clone)
}

impl IntlHost {
    /// Builds — or finds — the formatter for one set of options.
    ///
    /// Returns the handle and the options that were honoured, which is what
    /// `resolvedOptions()` answers with.
    pub(super) fn resolve(&self, kind: &str, options: &Value) -> Result<Value, JsError> {
        let key = format!("{kind}\u{1f}{options}");
        if let Some(&handle) = self.handles.borrow().get(&key) {
            let formatters = self.formatters.borrow();
            return Ok(json!({ "handle": handle, "resolved": formatters[handle].1 }));
        }
        let (formatter, resolved) = match kind {
            "number" => {
                let (formatter, resolved) = number::NumberFormat::new(options)?;
                (Formatter::Number(formatter), resolved)
            }
            "datetime" => {
                let (formatter, resolved) = datetime::DateTimeFormat::new(options)?;
                (Formatter::DateTime(Box::new(formatter)), resolved)
            }
            "relativetime" => {
                let (formatter, resolved) = datetime::RelativeTimeFormat::new(options)?;
                (Formatter::RelativeTime(formatter), resolved)
            }
            "plural" => {
                let (formatter, resolved) = text::PluralRules::new(options)?;
                (Formatter::Plural(formatter), resolved)
            }
            "collator" => {
                let (formatter, resolved) = text::Collator::new(options)?;
                (Formatter::Collator(formatter), resolved)
            }
            "list" => {
                let (formatter, resolved) = text::ListFormat::new(options)?;
                (Formatter::List(formatter), resolved)
            }
            other => return Err(JsError::new(format!("unknown Intl formatter: {other}"))),
        };
        let mut formatters = self.formatters.borrow_mut();
        let handle = formatters.len();
        formatters.push((formatter, resolved.clone()));
        drop(formatters);
        self.handles.borrow_mut().insert(key, handle);
        Ok(json!({ "handle": handle, "resolved": resolved }))
    }

    /// Formats one value with the formatter behind a handle.
    ///
    /// The value is a string for a number — the decimal the application meant,
    /// rather than the binary double it is stored as — and the milliseconds
    /// since the epoch for a date.
    pub(super) fn format(&self, handle: usize, value: &str) -> Result<String, JsError> {
        match &*self.formatter(handle)? {
            Formatter::Number(formatter) => formatter.format(value),
            Formatter::DateTime(formatter) => formatter.format(value),
            Formatter::RelativeTime(formatter) => formatter.format(value),
            _ => Err(JsError::new("this Intl formatter does not format a value")),
        }
    }

    /// Answers `PluralRules.select`, and `Collator.compare` when two are given.
    pub(super) fn select(&self, handle: usize, value: &str) -> Result<String, JsError> {
        match &*self.formatter(handle)? {
            Formatter::Plural(rules) => Ok(rules.select(value)),
            _ => Err(JsError::new("this Intl formatter does not select a value")),
        }
    }

    /// Answers `Collator.compare`, as the sign of the comparison.
    pub(super) fn compare(&self, handle: usize, left: &str, right: &str) -> Result<i8, JsError> {
        match &*self.formatter(handle)? {
            Formatter::Collator(collator) => Ok(collator.compare(left, right)),
            _ => Err(JsError::new("this Intl formatter does not compare")),
        }
    }

    /// Answers `ListFormat.format`.
    pub(super) fn join(&self, handle: usize, items: &[String]) -> Result<String, JsError> {
        match &*self.formatter(handle)? {
            Formatter::List(list) => Ok(list.format(items)),
            _ => Err(JsError::new("this Intl formatter does not join a list")),
        }
    }

    fn formatter(&self, handle: usize) -> Result<std::cell::Ref<'_, Formatter>, JsError> {
        let formatters = self.formatters.borrow();
        if handle >= formatters.len() {
            return Err(JsError::new("no such Intl formatter"));
        }
        Ok(std::cell::Ref::map(formatters, |formatters| {
            &formatters[handle].0
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolves a formatter and returns the host with its handle.
    fn formatter(host: &IntlHost, kind: &str, options: Value) -> usize {
        host.resolve(kind, &options)
            .unwrap_or_else(|error| panic!("{kind} {options}: {}", error.message()))["handle"]
            .as_u64()
            .expect("handle") as usize
    }

    fn format(host: &IntlHost, kind: &str, options: Value, value: &str) -> String {
        let handle = formatter(host, kind, options);
        host.format(handle, value).expect("format")
    }

    #[test]
    fn a_number_carries_the_separators_of_its_locale() {
        let host = IntlHost::default();
        assert_eq!(
            format(&host, "number", json!({ "locale": "de-DE" }), "1234567.891"),
            "1.234.567,891"
        );
        assert_eq!(
            format(&host, "number", json!({ "locale": "en-US" }), "1234567.891"),
            "1,234,567.891"
        );
        assert_eq!(
            format(
                &host,
                "number",
                json!({ "locale": "en-US", "useGrouping": false }),
                "1234567.891"
            ),
            "1234567.891"
        );
        assert_eq!(
            format(
                &host,
                "number",
                json!({ "locale": "en-US", "minimumFractionDigits": 2 }),
                "5"
            ),
            "5.00",
            "a minimum pads rather than rounds"
        );
        assert_eq!(
            format(
                &host,
                "number",
                json!({ "locale": "en-US", "maximumFractionDigits": 2 }),
                "2.345"
            ),
            "2.35",
            "ECMA-402 rounds half away from zero, not half to even"
        );
    }

    /// The case the issue is about: a currency's minor units are the currency's.
    #[test]
    fn a_currency_is_formatted_with_the_minor_units_it_has() {
        let host = IntlHost::default();
        let dollars = json!({ "locale": "en-US", "style": "currency", "currency": "USD" });
        assert_eq!(
            format(&host, "number", dollars.clone(), "1234.5"),
            "$1,234.50"
        );
        let yen = json!({ "locale": "en-US", "style": "currency", "currency": "JPY" });
        assert_eq!(
            format(&host, "number", yen.clone(), "1234.5"),
            "¥1,235",
            "the yen has no minor unit, so there is nothing to show two digits of"
        );
        assert_eq!(
            format(
                &host,
                "number",
                json!({ "locale": "de-DE", "style": "currency", "currency": "EUR" }),
                "1234.5"
            ),
            "1.234,50\u{a0}€",
            "placement and spacing are the locale's, not the currency's"
        );
        let resolved = host.resolve("number", &yen).expect("resolve")["resolved"].clone();
        assert_eq!(resolved["maximumFractionDigits"], 0);
        assert_eq!(
            host.resolve("number", &dollars).expect("resolve")["resolved"]["maximumFractionDigits"],
            2,
            "resolvedOptions reports the digits that were used"
        );
    }

    #[test]
    fn a_percentage_scales_and_takes_the_locales_own_sign_placement() {
        let host = IntlHost::default();
        let options = |locale: &str| json!({ "locale": locale, "style": "percent" });
        assert_eq!(format(&host, "number", options("en-US"), "0.256"), "26%");
        assert_eq!(
            format(&host, "number", options("fr-FR"), "0.256"),
            "26\u{a0}%",
            "French puts a non-breaking space before the sign"
        );
    }

    #[test]
    fn compact_notation_shortens_the_number_the_way_the_locale_does() {
        let host = IntlHost::default();
        let options = |locale: &str| json!({ "locale": locale, "notation": "compact" });
        assert_eq!(format(&host, "number", options("en-US"), "1234567"), "1.2M");
        assert_eq!(
            format(&host, "number", options("de-DE"), "1234567"),
            "1,2\u{a0}Mio."
        );
    }

    /// The case that blocks the issue's author: a named IANA zone, across the
    /// daylight-saving change that the user's own zone does not share.
    #[test]
    fn an_instant_is_rendered_in_the_named_zone_it_was_asked_for() {
        let host = IntlHost::default();
        // 2026-01-15T14:30:00Z and 2026-07-15T13:30:00Z: the same wall clock in
        // New York, one in standard time and one in daylight time.
        let winter = "1768487400000";
        let summer = "1784122200000";
        let options = |zone: &str| {
            json!({
                "locale": "en-US", "timeZone": zone,
                "dateStyle": "medium", "timeStyle": "long",
            })
        };
        assert_eq!(
            format(&host, "datetime", options("America/New_York"), winter),
            "Jan 15, 2026, 9:30:00\u{202f}AM EST"
        );
        assert_eq!(
            format(&host, "datetime", options("America/New_York"), summer),
            "Jul 15, 2026, 9:30:00\u{202f}AM EDT",
            "the offset follows the zone's own daylight-saving schedule"
        );
        assert_eq!(
            format(&host, "datetime", options("UTC"), winter),
            "Jan 15, 2026, 2:30:00\u{202f}PM UTC"
        );
        assert!(
            host.resolve("datetime", &json!({ "timeZone": "Mars/Olympus" }))
                .unwrap_err()
                .message()
                .contains("invalid time zone"),
            "a zone that is not in the database is refused rather than ignored"
        );
    }

    #[test]
    fn a_date_takes_the_field_shapes_and_names_of_its_locale() {
        let host = IntlHost::default();
        let instant = "1768487400000";
        let options = |locale: &str| {
            json!({
                "locale": locale, "timeZone": "UTC",
                "weekday": "long", "year": "numeric", "month": "long", "day": "numeric",
            })
        };
        assert_eq!(
            format(&host, "datetime", options("en-GB"), instant),
            "Thursday, 15 January 2026"
        );
        assert_eq!(
            format(&host, "datetime", options("de-DE"), instant),
            "Donnerstag, 15. Januar 2026"
        );
    }

    #[test]
    fn a_relative_time_is_worded_by_its_locale_and_its_numeric_option() {
        let host = IntlHost::default();
        let options = |locale: &str, numeric: &str| json!({ "locale": locale, "numeric": numeric });
        assert_eq!(
            format(
                &host,
                "relativetime",
                options("en-US", "auto"),
                "-1\u{1f}day"
            ),
            "yesterday"
        );
        assert_eq!(
            format(
                &host,
                "relativetime",
                options("en-US", "always"),
                "-1\u{1f}day"
            ),
            "1 day ago"
        );
        assert_eq!(
            format(
                &host,
                "relativetime",
                options("es-ES", "always"),
                "3\u{1f}minutes"
            ),
            "dentro de 3 minutos",
            "the plural unit an application passes is the singular one ICU4X names"
        );
    }

    #[test]
    fn plural_categories_are_the_locales_own_rule_set() {
        let host = IntlHost::default();
        let select = |locale: &str, value: &str| {
            let handle = formatter(&host, "plural", json!({ "locale": locale }));
            host.select(handle, value).expect("select")
        };
        assert_eq!(select("en-US", "1"), "one");
        assert_eq!(select("en-US", "2"), "other");
        assert_eq!(
            select("en-US", "1.0"),
            "other",
            "the digits written decide the category, which a double could not tell apart"
        );
        // Polish has three cardinal categories where English has two.
        assert_eq!(select("pl-PL", "2"), "few");
        assert_eq!(select("pl-PL", "5"), "many");
    }

    #[test]
    fn collation_orders_by_the_locales_table_rather_than_by_code_unit() {
        let host = IntlHost::default();
        let compare = |locale: &str, options: Value, left: &str, right: &str| {
            let mut options = options;
            options["locale"] = json!(locale);
            let handle = formatter(&host, "collator", options);
            host.compare(handle, left, right).expect("compare")
        };
        assert_eq!(
            compare("de-DE", json!({}), "ä", "z"),
            -1,
            "German sorts a-umlaut with a"
        );
        assert_eq!(
            compare("sv-SE", json!({}), "ä", "z"),
            1,
            "Swedish sorts it after z, which is the whole reason localeCompare exists"
        );
        assert_eq!(
            compare(
                "en-US",
                json!({ "sensitivity": "base" }),
                "resume",
                "résumé"
            ),
            0,
            "base sensitivity ignores the accents"
        );
        assert_eq!(
            compare("en-US", json!({ "numeric": true }), "item10", "item9"),
            1,
            "numeric collation compares the numbers, not the digits"
        );
    }

    #[test]
    fn a_list_is_joined_with_the_locales_own_pattern() {
        let host = IntlHost::default();
        let join = |locale: &str, kind: &str| {
            let handle = formatter(&host, "list", json!({ "locale": locale, "type": kind }));
            host.join(handle, &["a".to_owned(), "b".to_owned(), "c".to_owned()])
                .expect("join")
        };
        assert_eq!(join("en-US", "conjunction"), "a, b, and c");
        assert_eq!(join("en-US", "disjunction"), "a, b, or c");
        assert_eq!(join("de-DE", "conjunction"), "a, b und c");
    }

    #[test]
    fn the_same_options_asked_for_twice_are_one_formatter() {
        let host = IntlHost::default();
        let options = json!({ "locale": "en-US", "style": "percent" });
        let first = formatter(&host, "number", options.clone());
        let second = formatter(&host, "number", options);
        assert_eq!(
            first, second,
            "a formatter built in a render path is a map lookup the second time"
        );
        let other = formatter(&host, "number", json!({ "locale": "en-US" }));
        assert_ne!(first, other);
    }
}
