//! Source Map v3 positions used by runtime diagnostics.
//!
//! Blitsen never transforms application code. The mature `sourcemap` crate
//! owns JSON, VLQ and indexed-map decoding; this module keeps the one policy
//! that belongs to the application loader: resolving an original source name
//! against the URL from which its map was loaded.

use sourcemap::DecodedMap;
use url::Url;

#[derive(Debug)]
pub(crate) struct SourceMap {
    decoded: DecodedMap,
    map_url: String,
}

impl SourceMap {
    pub(crate) fn parse(bytes: &[u8], map_url: &str) -> Result<Self, String> {
        // Validate the base here rather than waiting for the first diagnostic.
        // A malformed optional map remains non-fatal to module loading, but its
        // cache entry must never become a delayed error on the reporting path.
        Url::parse(map_url)
            .map_err(|error| format!("invalid source-map URL {map_url:?}: {error}"))?;
        let version = serde_json::from_slice::<serde_json::Value>(bytes)
            .map_err(|error| format!("invalid source-map JSON: {error}"))?
            .get("version")
            .and_then(serde_json::Value::as_u64);
        if version != Some(3) {
            return Err(format!(
                "unsupported source-map version {}",
                version
                    .map(|version| version.to_string())
                    .unwrap_or_else(|| "missing".to_owned())
            ));
        }
        let decoded = sourcemap::decode_slice(bytes)
            .map_err(|error| format!("invalid source map: {error}"))?;
        // Flattening makes section offsets part of each token's generated
        // coordinates, so the same-line guard in `original_position` applies
        // equally to regular and indexed maps.
        let decoded = match decoded {
            DecodedMap::Index(index) => DecodedMap::Regular(
                index
                    .flatten()
                    .map_err(|error| format!("invalid indexed source map: {error}"))?,
            ),
            decoded => decoded,
        };
        if let DecodedMap::Regular(map) = &decoded
            && map
                .tokens()
                .any(|token| token.has_source() && token.get_source().is_none())
        {
            return Err("mapping source index is out of range".to_owned());
        }
        Ok(Self {
            decoded,
            map_url: map_url.to_owned(),
        })
    }

    /// Source-map inputs use zero-based positions; diagnostics use one-based
    /// line and column numbers.
    pub(crate) fn original_position(
        &self,
        generated_line: u32,
        generated_column: u32,
    ) -> Option<(String, u32, u32)> {
        let line = generated_line.checked_sub(1)?;
        let column = generated_column.saturating_sub(1);
        let token = self.decoded.lookup_token(line, column)?;
        // `lookup_token` is a greatest-lower-bound lookup over the whole map.
        // A token from the preceding generated line must not make an unmapped
        // line appear mapped.
        if token.get_dst_line() != line {
            return None;
        }
        let source = source_url(&self.map_url, token.get_source()?).ok()?;
        Some((
            source,
            token.get_src_line().checked_add(1)?,
            token.get_src_col().checked_add(1)?,
        ))
    }
}

fn source_url(map_url: &str, source: &str) -> Result<String, String> {
    if let Ok(url) = Url::parse(source) {
        return Ok(url.into());
    }
    Url::parse(map_url)
        .map_err(|error| format!("invalid source-map URL {map_url:?}: {error}"))?
        .join(source)
        .map(Into::into)
        .map_err(|error| format!("invalid source URL {source:?}: {error}"))
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
            Some(("blitsen://app/source/main.ts".to_owned(), 21, 6))
        );
    }

    #[test]
    fn embedded_indexed_maps_respect_section_offsets_and_source_roots() {
        let map = SourceMap::parse(
            br#"{
                "version": 3,
                "sections": [
                    {
                        "offset": { "line": 0, "column": 0 },
                        "map": {
                            "version": 3,
                            "sourceRoot": "../source",
                            "sources": ["first.ts"],
                            "names": [],
                            "mappings": "AAAA"
                        }
                    },
                    {
                        "offset": { "line": 2, "column": 4 },
                        "map": {
                            "version": 3,
                            "sources": ["second.ts"],
                            "names": [],
                            "mappings": "AAAA"
                        }
                    }
                ]
            }"#,
            "blitsen://app/assets/generated.js.map",
        )
        .unwrap();
        assert_eq!(
            map.original_position(1, 1),
            Some(("blitsen://app/source/first.ts".to_owned(), 1, 1))
        );
        assert_eq!(
            map.original_position(3, 5),
            Some(("blitsen://app/assets/second.ts".to_owned(), 1, 1))
        );
    }

    #[test]
    fn invalid_and_unmapped_maps_are_rejected_without_panicking() {
        for map in [
            br#"{"version":2,"sources":[],"mappings":""}"#.as_slice(),
            br#"{"version":3,"sources":["x.ts"],"mappings":"AA?A"}"#.as_slice(),
            br#"{"version":3,"sources":[],"mappings":"AAAA"}"#.as_slice(),
        ] {
            assert!(SourceMap::parse(map, "blitsen://app/x.js.map").is_err());
        }

        let map = SourceMap::parse(
            br#"{"version":3,"sources":["x.ts"],"names":[],"mappings":"AAAA;;"}"#,
            "blitsen://app/x.js.map",
        )
        .unwrap();
        assert_eq!(map.original_position(2, 1), None);
    }
}
