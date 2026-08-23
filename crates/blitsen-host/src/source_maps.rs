//! The small part of Source Map v3 a runtime diagnostic needs.
//!
//! Blitsen never transforms application code. It only needs to consume the
//! mappings emitted by the user's toolchain, so this deliberately stores the
//! generated-to-original positions and none of the source text or symbol-name
//! table a debugger would need.

use serde::Deserialize;
use url::Url;

#[derive(Debug)]
pub(crate) struct SourceMap {
    lines: Vec<Vec<Mapping>>,
    sources: Vec<String>,
}

#[derive(Debug)]
struct Mapping {
    generated_column: u32,
    source: usize,
    original_line: u32,
    original_column: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSourceMap {
    version: u8,
    #[serde(default)]
    source_root: Option<String>,
    sources: Vec<String>,
    mappings: String,
}

impl SourceMap {
    pub(crate) fn parse(bytes: &[u8], map_url: &str) -> Result<Self, String> {
        let raw: RawSourceMap =
            serde_json::from_slice(bytes).map_err(|error| format!("invalid JSON: {error}"))?;
        if raw.version != 3 {
            return Err(format!("unsupported source-map version {}", raw.version));
        }
        let sources = raw
            .sources
            .iter()
            .map(|source| source_url(map_url, raw.source_root.as_deref(), source))
            .collect::<Result<Vec<_>, _>>()?;
        let lines = decode_mappings(&raw.mappings, sources.len())?;
        Ok(Self { lines, sources })
    }

    /// Source-map inputs use zero-based positions; diagnostics use one-based
    /// line and column numbers.
    pub(crate) fn original_position(
        &self,
        generated_line: u32,
        generated_column: u32,
    ) -> Option<(&str, u32, u32)> {
        let line = self.lines.get(generated_line.checked_sub(1)? as usize)?;
        let column = generated_column.saturating_sub(1);
        let mapping = line
            .iter()
            .rev()
            .find(|mapping| mapping.generated_column <= column)?;
        Some((
            &self.sources[mapping.source],
            mapping.original_line + 1,
            mapping.original_column + 1,
        ))
    }
}

fn source_url(map_url: &str, source_root: Option<&str>, source: &str) -> Result<String, String> {
    let source = match source_root.filter(|root| !root.is_empty()) {
        Some(root) => format!(
            "{}/{}",
            root.trim_end_matches('/'),
            source.trim_start_matches('/')
        ),
        None => source.to_owned(),
    };
    if let Ok(url) = Url::parse(&source) {
        return Ok(url.into());
    }
    Url::parse(map_url)
        .map_err(|error| format!("invalid source-map URL {map_url:?}: {error}"))?
        .join(&source)
        .map(Into::into)
        .map_err(|error| format!("invalid source URL {source:?}: {error}"))
}

fn decode_mappings(encoded: &str, source_count: usize) -> Result<Vec<Vec<Mapping>>, String> {
    let mut lines = Vec::new();
    let mut previous_source = 0_i64;
    let mut previous_original_line = 0_i64;
    let mut previous_original_column = 0_i64;
    let mut previous_name = 0_i64;

    for encoded_line in encoded.split(';') {
        let mut line = Vec::new();
        let mut generated_column = 0_i64;
        for segment in encoded_line
            .split(',')
            .filter(|segment| !segment.is_empty())
        {
            let fields = decode_segment(segment)?;
            if fields.len() != 1 && fields.len() != 4 && fields.len() != 5 {
                return Err(format!(
                    "mapping segment has {} fields instead of 1, 4, or 5",
                    fields.len()
                ));
            }
            generated_column = checked_delta(generated_column, fields[0], "generated column")?;
            if fields.len() == 1 {
                continue;
            }
            previous_source = checked_delta(previous_source, fields[1], "source index")?;
            previous_original_line =
                checked_delta(previous_original_line, fields[2], "original line")?;
            previous_original_column =
                checked_delta(previous_original_column, fields[3], "original column")?;
            if fields.len() == 5 {
                previous_name = checked_delta(previous_name, fields[4], "name index")?;
            }
            let source = usize::try_from(previous_source)
                .ok()
                .filter(|source| *source < source_count)
                .ok_or_else(|| "mapping source index is out of range".to_owned())?;
            line.push(Mapping {
                generated_column: generated_column as u32,
                source,
                original_line: previous_original_line as u32,
                original_column: previous_original_column as u32,
            });
        }
        lines.push(line);
    }
    Ok(lines)
}

fn checked_delta(previous: i64, delta: i64, field: &str) -> Result<i64, String> {
    let value = previous
        .checked_add(delta)
        .filter(|value| (0..=u32::MAX as i64).contains(value))
        .ok_or_else(|| format!("invalid {field} delta"))?;
    Ok(value)
}

fn decode_segment(segment: &str) -> Result<Vec<i64>, String> {
    let mut fields = Vec::new();
    let mut value = 0_u64;
    let mut shift = 0_u32;
    for byte in segment.bytes() {
        let digit = base64_digit(byte)
            .ok_or_else(|| format!("invalid base64-VLQ character {:?}", char::from(byte)))?;
        value |= u64::from(digit & 31)
            .checked_shl(shift)
            .ok_or_else(|| "base64-VLQ value is too large".to_owned())?;
        if digit & 32 == 0 {
            let magnitude = value >> 1;
            let signed =
                i64::try_from(magnitude).map_err(|_| "base64-VLQ value is too large".to_owned())?;
            fields.push(if value & 1 == 1 { -signed } else { signed });
            value = 0;
            shift = 0;
        } else {
            shift = shift
                .checked_add(5)
                .filter(|shift| *shift < 64)
                .ok_or_else(|| "base64-VLQ value is too large".to_owned())?;
        }
    }
    if shift != 0 {
        return Err("unterminated base64-VLQ value".to_owned());
    }
    Ok(fields)
}

fn base64_digit(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_delta_mappings_and_resolves_sources_against_the_map() {
        let map = SourceMap::parse(
            br#"{"version":3,"sourceRoot":"../source","sources":["main.ts"],"mappings":"AAAA;AAoBK"}"#,
            "blitsen://app/assets/main.js.map?v=2",
        )
        .unwrap();
        assert_eq!(
            map.original_position(2, 18),
            Some(("blitsen://app/source/main.ts", 21, 6))
        );
    }

    #[test]
    fn invalid_maps_are_rejected_without_panicking() {
        for map in [
            br#"{"version":2,"sources":[],"mappings":""}"#.as_slice(),
            br#"{"version":3,"sources":["x.ts"],"mappings":"AA?A"}"#.as_slice(),
            br#"{"version":3,"sources":[],"mappings":"AAAA"}"#.as_slice(),
        ] {
            assert!(SourceMap::parse(map, "blitsen://app/x.js.map").is_err());
        }
    }
}
