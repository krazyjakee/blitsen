//! `Intl.DateTimeFormat` and `Intl.RelativeTimeFormat`.
//!
//! The time zone is the part with no substitute. `Date` can answer in UTC and
//! in whatever zone the host is in, and nothing else: converting an instant
//! into `America/New_York` needs the zone's history of offsets, including the
//! daylight-saving rules that changed in 2007 and the ones that will change
//! again. That history is the platform's `tzdb`, read here through `jiff`, and
//! the names and patterns wrapped around it are CLDR's, read through ICU4X.
//!
//! An instant crosses the boundary as milliseconds since the epoch — what
//! `Date.prototype.getTime` returns — so nothing about the conversion happens
//! twice or happens in JavaScript.

use std::cell::RefCell;
use std::collections::HashMap;

use blitsen_js::JsError;
use fixed_decimal::Decimal;
use icu_datetime::DateTimeFormatter;
use icu_datetime::fieldsets::builder::{DateFields, FieldSetBuilder, ZoneStyle};
use icu_datetime::fieldsets::enums::CompositeFieldSet;
use icu_datetime::options::{Length, TimePrecision};
use icu_experimental::relativetime::RelativeTimeFormatter;
use icu_experimental::relativetime::options::RelativeTimeFormatterOptions;
use icu_locale_core::Locale;
use icu_locale_core::preferences::extensions::unicode::keywords::HourCycle;
use serde_json::{Value, json};
use writeable::Writeable as _;

use super::{default_time_zone, locale, option};

pub(super) struct DateTimeFormat {
    formatter: DateTimeFormatter<CompositeFieldSet>,
    zone: jiff::tz::TimeZone,
}

/// Maps a component option to the length that produces it.
///
/// ECMA-402 asks for a month by shape — numeric, abbreviated, wide — and ICU4X
/// asks for a length that the pattern data then interprets per field. They meet
/// here rather than in the bootstrap, because which lengths exist is data.
fn length_for(options: &Value) -> Length {
    match (
        option(options, "dateStyle"),
        option(options, "month"),
        option(options, "weekday"),
    ) {
        (Some("full" | "long"), _, _) | (_, Some("long"), _) | (_, _, Some("long")) => Length::Long,
        (Some("medium"), _, _) | (_, Some("short"), _) | (_, _, Some("short")) => Length::Medium,
        (Some("short"), _, _) => Length::Short,
        (_, Some("numeric" | "2-digit"), _) => Length::Short,
        _ => Length::Medium,
    }
}

/// Chooses the set of date fields the options add up to.
fn date_fields(options: &Value) -> Option<DateFields> {
    if let Some(style) = option(options, "dateStyle") {
        return Some(if style == "full" {
            DateFields::YMDE
        } else {
            DateFields::YMD
        });
    }
    let weekday = options.get("weekday").is_some();
    let year = options.get("year").is_some();
    let month = options.get("month").is_some();
    let day = options.get("day").is_some();
    Some(match (weekday, year, month, day) {
        (false, false, false, false) => return None,
        (true, true, true, true) => DateFields::YMDE,
        (false, true, true, true) => DateFields::YMD,
        (true, false, true, true) => DateFields::MDE,
        (false, false, true, true) => DateFields::MD,
        (true, false, false, true) => DateFields::DE,
        (false, false, false, true) => DateFields::D,
        (true, false, false, false) => DateFields::E,
        (false, true, true, false) => DateFields::YM,
        (false, false, true, false) => DateFields::M,
        (false, true, false, false) => DateFields::Y,
        // Anything else names a combination CLDR has no skeleton for — a year
        // and a day with no month, say. The nearest whole date is what a
        // browser falls back to as well.
        _ => DateFields::YMD,
    })
}

/// Chooses how much of the time of day to show.
fn time_precision(options: &Value) -> Option<TimePrecision> {
    if let Some(style) = option(options, "timeStyle") {
        return Some(match style {
            "short" => TimePrecision::Minute,
            _ => TimePrecision::Second,
        });
    }
    let hour = options.get("hour").is_some();
    let minute = options.get("minute").is_some();
    let second = options.get("second").is_some();
    match (hour, minute, second) {
        (false, false, false) => None,
        (_, _, true) => Some(TimePrecision::Second),
        (_, true, false) => Some(TimePrecision::Minute),
        _ => Some(TimePrecision::Hour),
    }
}

/// Chooses how the zone is named, which `timeStyle` also decides.
fn zone_style(options: &Value) -> Option<ZoneStyle> {
    if let Some(name) = option(options, "timeZoneName") {
        return Some(match name {
            "long" => ZoneStyle::SpecificLong,
            "shortOffset" => ZoneStyle::LocalizedOffsetShort,
            "longOffset" => ZoneStyle::LocalizedOffsetLong,
            "shortGeneric" => ZoneStyle::GenericShort,
            "longGeneric" => ZoneStyle::GenericLong,
            _ => ZoneStyle::SpecificShort,
        });
    }
    match option(options, "timeStyle") {
        Some("full") => Some(ZoneStyle::SpecificLong),
        Some("long") => Some(ZoneStyle::SpecificShort),
        _ => None,
    }
}

impl DateTimeFormat {
    pub(super) fn new(options: &Value) -> Result<(Self, Value), JsError> {
        let locale = locale(options)?;
        let zone_name = option(options, "timeZone")
            .map(str::to_owned)
            .unwrap_or_else(default_time_zone);
        let zone = jiff::tz::TimeZone::get(&zone_name)
            .map_err(|_| JsError::new(format!("invalid time zone: {zone_name}")))?;

        let mut builder = FieldSetBuilder::new();
        builder.length = Some(length_for(options));
        builder.date_fields = date_fields(options);
        builder.time_precision = time_precision(options);
        builder.zone_style = zone_style(options);
        // A request for nothing at all is a date, which is what `new
        // Intl.DateTimeFormat()` with no options means.
        if builder.date_fields.is_none()
            && builder.time_precision.is_none()
            && builder.zone_style.is_none()
        {
            builder.date_fields = Some(DateFields::YMD);
        }
        let field_set = builder
            .build_composite()
            .map_err(|error| JsError::new(format!("unsupported date format: {error}")))?;

        let mut preferences: icu_datetime::DateTimeFormatterPreferences = (&locale).into();
        if let Some(cycle) = hour_cycle(options) {
            preferences.hour_cycle = Some(cycle);
        }
        let formatter = DateTimeFormatter::try_new(preferences, field_set)
            .map_err(|error| JsError::new(format!("no date data for this locale: {error}")))?;

        let mut resolved = json!({
            "locale": locale.to_string(),
            "timeZone": zone_name,
            "calendar": "gregory",
        });
        for key in [
            "dateStyle",
            "timeStyle",
            "weekday",
            "year",
            "month",
            "day",
            "hour",
            "minute",
            "second",
            "timeZoneName",
        ] {
            if let Some(value) = options.get(key) {
                resolved[key] = value.clone();
            }
        }
        if let Some(cycle) = option(options, "hourCycle") {
            resolved["hourCycle"] = json!(cycle);
        }
        Ok((Self { formatter, zone }, resolved))
    }

    pub(super) fn format(&self, value: &str) -> Result<String, JsError> {
        let milliseconds = value
            .parse::<f64>()
            .map_err(|_| JsError::new(format!("{value} is not a time Intl can format")))?;
        let timestamp = jiff::Timestamp::from_millisecond(milliseconds as i64)
            .map_err(|error| JsError::new(format!("time out of range: {error}")))?;
        let zoned = timestamp.to_zoned(self.zone.clone());
        Ok(self.formatter.format(&zoned).write_to_string().into_owned())
    }
}

/// Reads `hourCycle`, and the `hour12` shorthand that outranks it.
fn hour_cycle(options: &Value) -> Option<HourCycle> {
    match options.get("hour12").and_then(Value::as_bool) {
        Some(true) => return Some(HourCycle::H12),
        Some(false) => return Some(HourCycle::H23),
        None => {}
    }
    match option(options, "hourCycle") {
        Some("h11") => Some(HourCycle::H11),
        Some("h12") => Some(HourCycle::H12),
        Some("h23") => Some(HourCycle::H23),
        _ => None,
    }
}

/// One unit's formatter, built when that unit is first asked for.
///
/// ICU4X builds a formatter per unit and `Intl.RelativeTimeFormat` names the
/// unit at the call rather than at construction, so the eight of them are built
/// on demand: an application that only ever says "3 minutes ago" pays for one.
pub(super) struct RelativeTimeFormat {
    locale: Locale,
    style: String,
    options: RelativeTimeFormatterOptions,
    formatters: RefCell<HashMap<String, RelativeTimeFormatter>>,
}

impl RelativeTimeFormat {
    pub(super) fn new(options: &Value) -> Result<(Self, Value), JsError> {
        let locale = locale(options)?;
        let style = option(options, "style").unwrap_or("long").to_owned();
        let numeric = option(options, "numeric").unwrap_or("always");
        let resolved = json!({
            "locale": locale.to_string(),
            "style": style,
            "numeric": numeric,
        });
        Ok((
            Self {
                locale,
                style,
                options: {
                    let mut relative = RelativeTimeFormatterOptions::default();
                    relative.numeric = if numeric == "auto" {
                        icu_experimental::relativetime::options::Numeric::Auto
                    } else {
                        icu_experimental::relativetime::options::Numeric::Always
                    };
                    relative
                },
                formatters: RefCell::default(),
            },
            resolved,
        ))
    }

    /// Formats `"<value>\u{1f}<unit>"`, the pair the bootstrap sends.
    pub(super) fn format(&self, value: &str) -> Result<String, JsError> {
        let (amount, unit) = value
            .split_once('\u{1f}')
            .ok_or_else(|| JsError::new("a relative time is a value and a unit"))?;
        let amount = amount
            .parse::<Decimal>()
            .map_err(|_| JsError::new(format!("{amount} is not a number Intl can format")))?;
        let mut formatters = self.formatters.borrow_mut();
        if !formatters.contains_key(unit) {
            formatters.insert(unit.to_owned(), self.build(unit)?);
        }
        Ok(formatters
            .get(unit)
            .expect("just inserted")
            .format(amount)
            .write_to_string()
            .into_owned())
    }

    fn build(&self, unit: &str) -> Result<RelativeTimeFormatter, JsError> {
        let prefs = (&self.locale).into();
        let options = self.options;
        // The unit is singular in ICU4X and either in ECMA-402, where "day" and
        // "days" mean the same thing.
        let built = match (self.style.as_str(), unit.trim_end_matches('s')) {
            ("short", "second") => RelativeTimeFormatter::try_new_short_second(prefs, options),
            ("short", "minute") => RelativeTimeFormatter::try_new_short_minute(prefs, options),
            ("short", "hour") => RelativeTimeFormatter::try_new_short_hour(prefs, options),
            ("short", "day") => RelativeTimeFormatter::try_new_short_day(prefs, options),
            ("short", "week") => RelativeTimeFormatter::try_new_short_week(prefs, options),
            ("short", "month") => RelativeTimeFormatter::try_new_short_month(prefs, options),
            ("short", "quarter") => RelativeTimeFormatter::try_new_short_quarter(prefs, options),
            ("short", "year") => RelativeTimeFormatter::try_new_short_year(prefs, options),
            ("narrow", "second") => RelativeTimeFormatter::try_new_narrow_second(prefs, options),
            ("narrow", "minute") => RelativeTimeFormatter::try_new_narrow_minute(prefs, options),
            ("narrow", "hour") => RelativeTimeFormatter::try_new_narrow_hour(prefs, options),
            ("narrow", "day") => RelativeTimeFormatter::try_new_narrow_day(prefs, options),
            ("narrow", "week") => RelativeTimeFormatter::try_new_narrow_week(prefs, options),
            ("narrow", "month") => RelativeTimeFormatter::try_new_narrow_month(prefs, options),
            ("narrow", "quarter") => RelativeTimeFormatter::try_new_narrow_quarter(prefs, options),
            ("narrow", "year") => RelativeTimeFormatter::try_new_narrow_year(prefs, options),
            (_, "second") => RelativeTimeFormatter::try_new_long_second(prefs, options),
            (_, "minute") => RelativeTimeFormatter::try_new_long_minute(prefs, options),
            (_, "hour") => RelativeTimeFormatter::try_new_long_hour(prefs, options),
            (_, "day") => RelativeTimeFormatter::try_new_long_day(prefs, options),
            (_, "week") => RelativeTimeFormatter::try_new_long_week(prefs, options),
            (_, "month") => RelativeTimeFormatter::try_new_long_month(prefs, options),
            (_, "quarter") => RelativeTimeFormatter::try_new_long_quarter(prefs, options),
            (_, "year") => RelativeTimeFormatter::try_new_long_year(prefs, options),
            (_, other) => {
                return Err(JsError::new(format!("{other} is not a relative time unit")));
            }
        };
        built.map_err(|error| JsError::new(format!("no relative time data: {error}")))
    }
}
