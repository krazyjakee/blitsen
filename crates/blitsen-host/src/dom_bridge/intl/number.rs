//! `Intl.NumberFormat`: decimals, percentages, currencies and compact notation.
//!
//! The value arrives as the decimal text JavaScript would print, not as a
//! double, and is parsed into a decimal here. That is the difference between
//! formatting the number an application meant and formatting the binary
//! approximation it is stored in — and it is what lets the currency path round
//! to the minor units of the currency rather than to whatever the float had.

use fixed_decimal::{Decimal, Sign, SignedRoundingMode, UnsignedRoundingMode};
use icu_decimal::options::{DecimalFormatterOptions, GroupingStrategy};
use icu_decimal::{CompactDecimalFormatter, DecimalFormatter};
use icu_experimental::dimension::currency::CurrencyType;
use icu_experimental::dimension::currency::formatter::CurrencyFormatter;
use icu_experimental::dimension::percent::formatter::PercentFormatter;
use icu_locale_core::Locale;
use serde_json::{Value, json};
use writeable::Writeable as _;

use super::{count, flag, locale, option};
use blitsen_js::JsError;

/// How the number is decorated once it has been formatted.
enum Engine {
    Decimal(DecimalFormatter),
    Compact(CompactDecimalFormatter),
    Percent(PercentFormatter<DecimalFormatter>),
    Currency(Box<CurrencyFormatter<DecimalFormatter>>),
}

/// What `signDisplay` asked for, applied to the decimal before formatting.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SignDisplay {
    Auto,
    Never,
    Always,
    ExceptZero,
}

pub(super) struct NumberFormat {
    engine: Engine,
    /// `true` for `style: "percent"`, which scales before it decorates.
    scale_by_hundred: bool,
    minimum_integer_digits: u8,
    minimum_fraction_digits: u8,
    maximum_fraction_digits: u8,
    /// Set for currency and compact, where the digits are the data's decision
    /// rather than this module's and rounding here would fight it.
    digits_are_the_formatters: bool,
    sign_display: SignDisplay,
}

/// Reads the ECMA-402 defaults for a style, before the options are applied.
///
/// A decimal shows up to three fraction digits, a percentage none, and a
/// currency as many as its minor unit has — which is CLDR's answer, not one
/// this module holds a table of.
fn default_digits(style: &str) -> (u8, u8) {
    match style {
        "percent" => (0, 0),
        _ => (0, 3),
    }
}

/// The number of fraction digits a currency formatter actually produced.
///
/// Read out of a formatted sample rather than from a table: the currency's
/// minor units live in CLDR, ICU4X applies them itself, and a second copy here
/// would be a table to keep in step for no gain. The sample is one unit, which
/// every currency formats.
fn currency_fraction_digits(formatter: &CurrencyFormatter<DecimalFormatter>) -> u8 {
    let mut sink = Parts::default();
    let _ = formatter
        .format_fixed_decimal(&Decimal::from(1))
        .write_to_parts(&mut sink);
    sink.fraction_length()
}

/// Collects the spans ICU4X labels while writing, so a formatted sample can be
/// read back structurally rather than by scanning for a separator that is
/// itself locale data.
#[derive(Default)]
struct Parts {
    text: String,
    fraction: Option<(usize, usize)>,
}

impl Parts {
    fn fraction_length(&self) -> u8 {
        self.fraction
            .map(|(start, end)| self.text[start..end].chars().count().min(255) as u8)
            .unwrap_or_default()
    }
}

impl std::fmt::Write for Parts {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.text.push_str(text);
        Ok(())
    }
}

impl writeable::PartsWrite for Parts {
    type SubPartsWrite = Self;

    fn with_part(
        &mut self,
        part: writeable::Part,
        mut body: impl FnMut(&mut Self) -> std::fmt::Result,
    ) -> std::fmt::Result {
        let start = self.text.len();
        body(self)?;
        if part == icu_decimal::parts::FRACTION {
            self.fraction = Some((start, self.text.len()));
        }
        Ok(())
    }
}

/// Parses the currency an application named, which is three ASCII letters.
fn currency_code(options: &Value) -> Result<CurrencyType, JsError> {
    let code = option(options, "currency")
        .ok_or_else(|| JsError::new("currency is required when style is \"currency\""))?;
    CurrencyType::try_from_str(code)
        .map_err(|_| JsError::new(format!("{code} is not an ISO 4217 currency code")))
}

/// Builds the currency formatter the `currencyDisplay` option asked for.
///
/// Compact currency notation — `$12K` — is a separate family of constructors
/// over a compact value formatter, and is not built here: `notation: "compact"`
/// with `style: "currency"` formats the amount in full. See COMPATIBILITY.md.
fn currency_formatter(
    locale: &Locale,
    code: CurrencyType,
    display: &str,
) -> Result<CurrencyFormatter<DecimalFormatter>, JsError> {
    let prefs = locale.into();
    let options = Default::default();
    let built = match display {
        "code" => CurrencyFormatter::try_new_code(prefs, code, options),
        "name" => CurrencyFormatter::try_new_name(prefs, code),
        "narrowSymbol" => CurrencyFormatter::try_new_symbol_narrow(prefs, code, options),
        _ => CurrencyFormatter::try_new_symbol(prefs, code, options),
    };
    built.map_err(|error| JsError::new(format!("no currency data for this locale: {error}")))
}

impl NumberFormat {
    pub(super) fn new(options: &Value) -> Result<(Self, Value), JsError> {
        let locale = locale(options)?;
        let style = option(options, "style").unwrap_or("decimal");
        let compact = option(options, "notation") == Some("compact");
        let grouping = match flag(options, "useGrouping") {
            Some(false) => GroupingStrategy::Never,
            _ => GroupingStrategy::Auto,
        };
        let mut decimal_options = DecimalFormatterOptions::default();
        decimal_options.grouping_strategy = Some(grouping);

        let (engine, digits_are_the_formatters) = match (style, compact) {
            ("currency", _) => {
                let formatter = currency_formatter(
                    &locale,
                    currency_code(options)?,
                    option(options, "currencyDisplay").unwrap_or("symbol"),
                )?;
                (Engine::Currency(Box::new(formatter)), true)
            }
            ("percent", _) => (
                Engine::Percent(
                    PercentFormatter::try_new((&locale).into(), Default::default()).map_err(
                        |error| JsError::new(format!("no percent data for this locale: {error}")),
                    )?,
                ),
                false,
            ),
            (_, true) => {
                let long = option(options, "compactDisplay") == Some("long");
                let formatter = if long {
                    CompactDecimalFormatter::try_new_long((&locale).into(), Default::default())
                } else {
                    CompactDecimalFormatter::try_new_short((&locale).into(), Default::default())
                };
                (
                    Engine::Compact(formatter.map_err(|error| {
                        JsError::new(format!("no compact data for this locale: {error}"))
                    })?),
                    true,
                )
            }
            _ => (
                Engine::Decimal(
                    DecimalFormatter::try_new((&locale).into(), decimal_options).map_err(
                        |error| JsError::new(format!("no number data for this locale: {error}")),
                    )?,
                ),
                false,
            ),
        };

        let (default_minimum, default_maximum) = default_digits(style);
        let minimum_fraction_digits =
            count(options, "minimumFractionDigits").unwrap_or(default_minimum);
        let maximum_fraction_digits = count(options, "maximumFractionDigits")
            .unwrap_or(default_maximum.max(minimum_fraction_digits));
        let sign_display = match option(options, "signDisplay") {
            Some("never") => SignDisplay::Never,
            Some("always") => SignDisplay::Always,
            Some("exceptZero") => SignDisplay::ExceptZero,
            _ => SignDisplay::Auto,
        };
        let format = Self {
            engine,
            scale_by_hundred: style == "percent",
            minimum_integer_digits: count(options, "minimumIntegerDigits").unwrap_or(1).max(1),
            minimum_fraction_digits,
            maximum_fraction_digits: maximum_fraction_digits.max(minimum_fraction_digits),
            digits_are_the_formatters,
            sign_display,
        };

        // What is reported back is what was used. For a currency the digits are
        // the currency's, read out of the formatter; for compact notation they
        // are the compact pattern's, which no fixed pair describes, so the keys
        // are absent rather than invented.
        let mut resolved = json!({
            "locale": locale.to_string(),
            "style": style,
            "notation": if compact { "compact" } else { "standard" },
            "useGrouping": grouping != GroupingStrategy::Never,
            "minimumIntegerDigits": format.minimum_integer_digits,
            "signDisplay": match format.sign_display {
                SignDisplay::Never => "never",
                SignDisplay::Always => "always",
                SignDisplay::ExceptZero => "exceptZero",
                SignDisplay::Auto => "auto",
            },
        });
        match (&format.engine, compact) {
            (Engine::Currency(formatter), _) => {
                let digits = currency_fraction_digits(formatter);
                resolved["currency"] = json!(option(options, "currency"));
                resolved["currencyDisplay"] =
                    json!(option(options, "currencyDisplay").unwrap_or("symbol"));
                resolved["minimumFractionDigits"] = json!(digits);
                resolved["maximumFractionDigits"] = json!(digits);
            }
            (_, false) => {
                resolved["minimumFractionDigits"] = json!(format.minimum_fraction_digits);
                resolved["maximumFractionDigits"] = json!(format.maximum_fraction_digits);
            }
            (_, true) => {}
        }
        Ok((format, resolved))
    }

    pub(super) fn format(&self, value: &str) -> Result<String, JsError> {
        let mut decimal = value
            .parse::<Decimal>()
            .map_err(|_| JsError::new(format!("{value} is not a number Intl can format")))?;
        if self.scale_by_hundred {
            decimal.multiply_pow10(2);
        }
        if !self.digits_are_the_formatters {
            // ECMA-402 rounds half away from zero, which is the mode a spreadsheet
            // and an invoice both assume; ICU4X's own default is half-even.
            decimal.round_with_mode(
                -i16::from(self.maximum_fraction_digits),
                SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand),
            );
            decimal.pad_end(-i16::from(self.minimum_fraction_digits));
        }
        decimal.pad_start(i16::from(self.minimum_integer_digits));
        let is_zero = decimal.is_zero();
        match self.sign_display {
            SignDisplay::Auto => {}
            SignDisplay::Never => decimal.set_sign(Sign::None),
            SignDisplay::Always if decimal.sign() != Sign::Negative => {
                decimal.set_sign(Sign::Positive);
            }
            SignDisplay::ExceptZero if !is_zero && decimal.sign() != Sign::Negative => {
                decimal.set_sign(Sign::Positive);
            }
            _ => {}
        }
        Ok(match &self.engine {
            Engine::Decimal(formatter) => formatter.format(&decimal).write_to_string().into_owned(),
            Engine::Percent(formatter) => formatter.format(&decimal).write_to_string().into_owned(),
            Engine::Compact(formatter) => formatter.format(&decimal).write_to_string().into_owned(),
            Engine::Currency(formatter) => formatter
                .format_fixed_decimal(&decimal)
                .write_to_string()
                .into_owned(),
        })
    }
}
