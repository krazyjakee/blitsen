//! `Intl.PluralRules`, `Intl.Collator` and `Intl.ListFormat`.
//!
//! The three of them are why `localeCompare` and a hand-written pluraliser are
//! not substitutes rather than shortcuts: sort order is a per-locale collation
//! table, plural categories are a per-locale rule set with six of them in some
//! languages, and joining a list is a per-locale pattern that is not always a
//! comma. All three are CLDR data, read here through ICU4X.

use blitsen_js::JsError;
use fixed_decimal::Decimal;
use icu_collator::options::{AlternateHandling, CaseLevel, CollatorOptions, MaxVariable, Strength};
use icu_collator::preferences::{CollationCaseFirst, CollationNumericOrdering};
use icu_collator::{Collator as IcuCollator, CollatorPreferences};
use icu_list::ListFormatter;
use icu_list::options::{ListFormatterOptions, ListLength};
use icu_plurals::{PluralRuleType, PluralRules as IcuPluralRules};
use serde_json::{Value, json};
use writeable::Writeable as _;

use super::{flag, locale, option};

pub(super) struct PluralRules(IcuPluralRules);

impl PluralRules {
    pub(super) fn new(options: &Value) -> Result<(Self, Value), JsError> {
        let locale = locale(options)?;
        let ordinal = option(options, "type") == Some("ordinal");
        let rules = if ordinal {
            IcuPluralRules::try_new((&locale).into(), PluralRuleType::Ordinal.into())
        } else {
            IcuPluralRules::try_new((&locale).into(), PluralRuleType::Cardinal.into())
        }
        .map_err(|error| JsError::new(format!("no plural data for this locale: {error}")))?;
        let resolved = json!({
            "locale": locale.to_string(),
            "type": if ordinal { "ordinal" } else { "cardinal" },
        });
        Ok((Self(rules), resolved))
    }

    /// Selects the category for a number, which arrives as its decimal text.
    ///
    /// The text rather than a double, because the category depends on the
    /// digits written: English has one plural form for `1` and another for
    /// `1.0`, and a double cannot tell them apart.
    pub(super) fn select(&self, value: &str) -> String {
        let category = match value.parse::<Decimal>() {
            Ok(decimal) => self.0.category_for(&decimal),
            Err(_) => icu_plurals::PluralCategory::Other,
        };
        match category {
            icu_plurals::PluralCategory::Zero => "zero",
            icu_plurals::PluralCategory::One => "one",
            icu_plurals::PluralCategory::Two => "two",
            icu_plurals::PluralCategory::Few => "few",
            icu_plurals::PluralCategory::Many => "many",
            icu_plurals::PluralCategory::Other => "other",
        }
        .to_owned()
    }
}

pub(super) struct Collator(icu_collator::CollatorBorrowed<'static>);

impl Collator {
    pub(super) fn new(options: &Value) -> Result<(Self, Value), JsError> {
        let locale = locale(options)?;
        let sensitivity = option(options, "sensitivity").unwrap_or("variant");
        let ignore_punctuation = flag(options, "ignorePunctuation").unwrap_or(false);
        let numeric = flag(options, "numeric").unwrap_or(false);
        let case_first = option(options, "caseFirst").unwrap_or("false");

        let mut preferences: CollatorPreferences = (&locale).into();
        preferences.numeric_ordering = Some(if numeric {
            CollationNumericOrdering::True
        } else {
            CollationNumericOrdering::False
        });
        preferences.case_first = Some(match case_first {
            "upper" => CollationCaseFirst::Upper,
            "lower" => CollationCaseFirst::Lower,
            _ => CollationCaseFirst::False,
        });

        let mut collator_options = CollatorOptions::default();
        // ECMA-402's sensitivity is ICU's strength plus case level, and the two
        // meet exactly: "base" ignores accents and case, "accent" keeps accents,
        // "case" keeps case alone, "variant" keeps both.
        collator_options.strength = Some(match sensitivity {
            "base" => Strength::Primary,
            "accent" => Strength::Secondary,
            "case" => Strength::Primary,
            _ => Strength::Tertiary,
        });
        if sensitivity == "case" {
            collator_options.case_level = Some(CaseLevel::On);
        }
        if ignore_punctuation {
            collator_options.alternate_handling = Some(AlternateHandling::Shifted);
            collator_options.max_variable = Some(MaxVariable::Punctuation);
        }
        let collator = IcuCollator::try_new(preferences, collator_options)
            .map_err(|error| JsError::new(format!("no collation data: {error}")))?;
        let resolved = json!({
            "locale": locale.to_string(),
            "usage": option(options, "usage").unwrap_or("sort"),
            "sensitivity": sensitivity,
            "ignorePunctuation": ignore_punctuation,
            "numeric": numeric,
            "caseFirst": case_first,
        });
        Ok((Self(collator), resolved))
    }

    pub(super) fn compare(&self, left: &str, right: &str) -> i8 {
        match self.0.compare(left, right) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }
    }
}

pub(super) struct ListFormat(ListFormatter);

impl ListFormat {
    pub(super) fn new(options: &Value) -> Result<(Self, Value), JsError> {
        let locale = locale(options)?;
        let kind = option(options, "type").unwrap_or("conjunction");
        let style = option(options, "style").unwrap_or("long");
        let length = match style {
            "short" => ListLength::Short,
            "narrow" => ListLength::Narrow,
            _ => ListLength::Wide,
        };
        let list_options = ListFormatterOptions::default().with_length(length);
        let formatter = match kind {
            "disjunction" => ListFormatter::try_new_or((&locale).into(), list_options),
            "unit" => ListFormatter::try_new_unit((&locale).into(), list_options),
            _ => ListFormatter::try_new_and((&locale).into(), list_options),
        }
        .map_err(|error| JsError::new(format!("no list data for this locale: {error}")))?;
        let resolved = json!({
            "locale": locale.to_string(),
            "type": kind,
            "style": style,
        });
        Ok((Self(formatter), resolved))
    }

    pub(super) fn format(&self, items: &[String]) -> String {
        self.0.format(items.iter()).write_to_string().into_owned()
    }
}
