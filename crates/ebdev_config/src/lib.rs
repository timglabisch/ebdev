use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    Io(#[from] std::io::Error),

    #[error("No config file found (.ebdev.ts)")]
    NotFound,

    #[error("Failed to load TypeScript config: {0}")]
    TypeScript(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub toolchain: ToolchainConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolchainConfig {
    pub ebdev: EbdevSelfConfig,
    pub node: NodeConfig,
    #[serde(default)]
    pub pnpm: Option<PnpmConfig>,
    #[serde(default)]
    pub mutagen: Option<MutagenConfig>,
    #[serde(default)]
    pub rust: Option<RustConfig>,
    #[serde(default)]
    pub gh: Option<GhConfig>,
    #[serde(default)]
    pub binary: Option<HashMap<String, BinaryToolchainConfig>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PnpmConfig {
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutagenConfig {
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RustConfig {
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbdevSelfConfig {
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhConfig {
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryToolchainConfig {
    pub version: String,
    pub url: String,
    #[serde(default)]
    pub binary: Option<String>,
}

impl Config {
    /// Load config from a directory (looks for .ebdev.ts)
    pub async fn load_from_dir(dir: &Path) -> Result<Self, ConfigError> {
        let ts_path = dir.join(".ebdev.ts");
        if ts_path.exists() {
            return ebdev_toolchain_deno::load_ts_config(&ts_path).await
                .map_err(|e| ConfigError::TypeScript(e.to_string()));
        }
        Err(ConfigError::NotFound)
    }
}

/// Extract the ebdev version from raw `.ebdev.ts` source text without evaluating TypeScript.
///
/// This is a best-effort parser used as a fallback when the full Deno-based config
/// load fails (e.g., because the old binary's embedded types don't match a newer config).
/// It enables self-update even when the config can't be fully evaluated.
///
/// Handles:
/// - `ebdev: "0.2.0"` (string shorthand, double quotes)
/// - `ebdev: '0.2.0'` (single quotes)
/// - `ebdev: { version: "0.2.0" }` (object form, inline or multiline)
/// - Skips occurrences inside single-line (`//`) and block (`/* */`) comments
pub fn extract_ebdev_version(source: &str) -> Option<String> {
    let mut search_from = 0;

    loop {
        let idx = source[search_from..].find("ebdev")?;
        let abs_idx = search_from + idx;
        search_from = abs_idx + 5;

        // Skip if "ebdev" is part of a longer identifier (e.g., "myebdev", "_ebdev")
        if abs_idx > 0 {
            let prev = source.as_bytes()[abs_idx - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' {
                continue;
            }
        }

        // Skip occurrences inside comments
        if is_in_comment(source, abs_idx) {
            continue;
        }

        // Must be followed by optional whitespace and ':'
        let rest = source[search_from..].trim_start();
        if !rest.starts_with(':') {
            continue;
        }
        let after_colon = rest[1..].trim_start();

        // Try string form: ebdev: "0.2.0" or ebdev: '0.2.0'
        if let Some(version) = try_extract_quoted_version(after_colon) {
            return Some(version);
        }

        // Try object form: ebdev: { version: "0.2.0" }
        if after_colon.starts_with('{') {
            if let Some(version) = try_extract_object_version(after_colon) {
                return Some(version);
            }
        }
    }
}

fn is_in_comment(source: &str, pos: usize) -> bool {
    is_in_line_comment(source, pos) || is_in_block_comment(source, pos)
}

fn is_in_line_comment(source: &str, pos: usize) -> bool {
    let line_start = source[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_before = &source[line_start..pos];
    line_before.contains("//")
}

fn is_in_block_comment(source: &str, pos: usize) -> bool {
    let before = &source[..pos];
    let last_open = before.rfind("/*");
    let last_close = before.rfind("*/");
    match (last_open, last_close) {
        (Some(_), None) => true,
        (Some(open), Some(close)) => open > close,
        _ => false,
    }
}

fn try_extract_quoted_version(s: &str) -> Option<String> {
    let first = s.chars().next()?;
    if first != '"' && first != '\'' {
        return None;
    }
    let end = s[1..].find(first)?;
    let candidate = &s[1..1 + end];
    if looks_like_version(candidate) {
        Some(candidate.to_string())
    } else {
        None
    }
}

fn try_extract_object_version(s: &str) -> Option<String> {
    // s starts with '{'
    let brace_end = s.find('}')?;
    let inner = &s[1..brace_end];

    let ver_idx = inner.find("version")?;
    let after_ver = inner[ver_idx + 7..].trim_start();
    if !after_ver.starts_with(':') {
        return None;
    }
    let after_colon = after_ver[1..].trim_start();
    try_extract_quoted_version(after_colon)
}

fn looks_like_version(s: &str) -> bool {
    let main_part = s.split('-').next().unwrap_or(s);
    let parts: Vec<&str> = main_part.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── String shorthand ────────────────────────────────────────────────

    #[test]
    fn string_shorthand_double_quotes() {
        assert_eq!(
            extract_ebdev_version(r#"ebdev: "0.2.0""#),
            Some("0.2.0".into()),
        );
    }

    #[test]
    fn string_shorthand_single_quotes() {
        assert_eq!(
            extract_ebdev_version("ebdev: '1.0.0'"),
            Some("1.0.0".into()),
        );
    }

    #[test]
    fn string_shorthand_extra_whitespace() {
        assert_eq!(
            extract_ebdev_version(r#"ebdev  :  "0.3.0""#),
            Some("0.3.0".into()),
        );
    }

    #[test]
    fn string_shorthand_with_trailing_comma() {
        assert_eq!(
            extract_ebdev_version(r#"ebdev: "0.2.0","#),
            Some("0.2.0".into()),
        );
    }

    // ── Object form ─────────────────────────────────────────────────────

    #[test]
    fn object_form_inline() {
        assert_eq!(
            extract_ebdev_version(r#"ebdev: { version: "0.4.0" }"#),
            Some("0.4.0".into()),
        );
    }

    #[test]
    fn object_form_single_quotes() {
        assert_eq!(
            extract_ebdev_version("ebdev: { version: '0.5.0' }"),
            Some("0.5.0".into()),
        );
    }

    #[test]
    fn object_form_multiline() {
        let source = r#"
    ebdev: {
      version: "0.6.0"
    }
"#;
        assert_eq!(extract_ebdev_version(source), Some("0.6.0".into()));
    }

    #[test]
    fn object_form_extra_whitespace() {
        assert_eq!(
            extract_ebdev_version(r#"ebdev :  {  version :  "0.7.0"  }"#),
            Some("0.7.0".into()),
        );
    }

    // ── Pre-release versions ────────────────────────────────────────────

    #[test]
    fn prerelease_version() {
        assert_eq!(
            extract_ebdev_version(r#"ebdev: "0.2.0-beta.1""#),
            Some("0.2.0-beta.1".into()),
        );
    }

    #[test]
    fn prerelease_rc() {
        assert_eq!(
            extract_ebdev_version(r#"ebdev: "1.0.0-rc1""#),
            Some("1.0.0-rc1".into()),
        );
    }

    // ── Real-world config files ─────────────────────────────────────────

    #[test]
    fn real_config_simple() {
        let source = r#"import { defineConfig } from "ebdev";

export default defineConfig({
  toolchain: {
    ebdev: "0.1.0",
    node: "22.12.0",
    pnpm: "9.15.0",
    mutagen: "0.17.6",
  },
});
"#;
        assert_eq!(extract_ebdev_version(source), Some("0.1.0".into()));
    }

    #[test]
    fn real_config_with_flags() {
        let source = r#"import { defineConfig, defineTask, arg, flag, exec } from "ebdev";

const config = defineConfig({
    toolchain: {
        ebdev: "0.1.0",
        node: "22.12.0",
        pnpm: "9.15.0",
        mutagen: "0.18.1",
    },
    flags: {
        search: flag("Elasticsearch").config({
            engine: arg.oneOf(["elasticsearch", "meilisearch"] as const).default("elasticsearch"),
        }),
        clickhouse: flag("ClickHouse Analytics").default(true),
    },
});
export default config;
"#;
        assert_eq!(extract_ebdev_version(source), Some("0.1.0".into()));
    }

    // ── Comment handling ────────────────────────────────────────────────

    #[test]
    fn skips_line_comment() {
        let source = r#"
// ebdev: "9.9.9"
toolchain: {
    ebdev: "0.1.0",
}
"#;
        assert_eq!(extract_ebdev_version(source), Some("0.1.0".into()));
    }

    #[test]
    fn skips_inline_comment() {
        let source = r#"
const x = 1; // ebdev: "9.9.9"
toolchain: {
    ebdev: "0.1.0",
}
"#;
        assert_eq!(extract_ebdev_version(source), Some("0.1.0".into()));
    }

    #[test]
    fn skips_block_comment() {
        let source = r#"
/* ebdev: "9.9.9" */
toolchain: {
    ebdev: "0.1.0",
}
"#;
        assert_eq!(extract_ebdev_version(source), Some("0.1.0".into()));
    }

    #[test]
    fn skips_multiline_block_comment() {
        let source = r#"
/*
  Old config:
  ebdev: "9.9.9"
*/
toolchain: {
    ebdev: "0.1.0",
}
"#;
        assert_eq!(extract_ebdev_version(source), Some("0.1.0".into()));
    }

    #[test]
    fn only_in_comment_returns_none() {
        let source = r#"
// ebdev: "0.1.0"
node: "22.12.0"
"#;
        assert_eq!(extract_ebdev_version(source), None);
    }

    // ── Import line should not match ────────────────────────────────────

    #[test]
    fn import_line_not_matched() {
        // "ebdev" appears in import string — no ':' follows it outside quotes
        let source = r#"import { defineConfig } from "ebdev";
"#;
        assert_eq!(extract_ebdev_version(source), None);
    }

    #[test]
    fn import_plus_config() {
        let source = r#"import { defineConfig } from "ebdev";
export default defineConfig({
    toolchain: {
        ebdev: "0.3.0",
        node: "22.12.0",
    },
});
"#;
        assert_eq!(extract_ebdev_version(source), Some("0.3.0".into()));
    }

    // ── Non-matching cases ──────────────────────────────────────────────

    #[test]
    fn empty_string() {
        assert_eq!(extract_ebdev_version(""), None);
    }

    #[test]
    fn no_ebdev_field() {
        assert_eq!(
            extract_ebdev_version(r#"node: "22.12.0", pnpm: "9.15.0""#),
            None,
        );
    }

    #[test]
    fn ebdev_with_variable_value() {
        // ebdev: someVar — not a quoted string, should not match
        assert_eq!(extract_ebdev_version("ebdev: someVar"), None);
    }

    #[test]
    fn ebdev_with_non_version_string() {
        // "hello" is not a semver version
        assert_eq!(extract_ebdev_version(r#"ebdev: "hello""#), None);
    }

    #[test]
    fn ebdev_with_partial_version() {
        // "0.1" is not a full semver
        assert_eq!(extract_ebdev_version(r#"ebdev: "0.1""#), None);
    }

    // ── Indentation variants ────────────────────────────────────────────

    #[test]
    fn tab_indentation() {
        let source = "\ttoolchain: {\n\t\tebdev: \"0.2.0\",\n\t\tnode: \"22.0.0\",\n\t},";
        assert_eq!(extract_ebdev_version(source), Some("0.2.0".into()));
    }

    #[test]
    fn deep_nesting() {
        let source = r#"
export default defineConfig({
    toolchain: {
        ebdev: "0.8.0",
    },
});
"#;
        assert_eq!(extract_ebdev_version(source), Some("0.8.0".into()));
    }

    // ── Word boundary ───────────────────────────────────────────────────

    #[test]
    fn prefix_identifier_not_matched() {
        // "myebdev" contains "ebdev" but is a different identifier
        assert_eq!(extract_ebdev_version(r#"myebdev: "1.0.0""#), None);
    }

    #[test]
    fn underscore_prefix_not_matched() {
        assert_eq!(extract_ebdev_version(r#"_ebdev: "1.0.0""#), None);
    }

    #[test]
    fn digit_prefix_not_matched() {
        assert_eq!(extract_ebdev_version(r#"2ebdev: "1.0.0""#), None);
    }

    #[test]
    fn prefix_identifier_skipped_real_found() {
        // First "ebdev" is inside "myebdev" → skip, second is the real one
        let source = r#"
const myebdev = "tool";
toolchain: {
    ebdev: "0.5.0",
}
"#;
        assert_eq!(extract_ebdev_version(source), Some("0.5.0".into()));
    }

    // ── Loop continuation (first match invalid, second valid) ───────────

    #[test]
    fn first_match_non_version_second_valid() {
        // First ebdev: has a non-version string, loop continues to the second
        let source = r#"
const label = "ebdev";
ebdev: "not-semver",
ebdev: "0.3.0",
"#;
        // First "ebdev" is in string → no `:` after → skip
        // Second `ebdev: "not-semver"` → looks_like_version fails → skip
        // Third `ebdev: "0.3.0"` → match
        assert_eq!(extract_ebdev_version(source), Some("0.3.0".into()));
    }

    #[test]
    fn ebdev_as_variable_name() {
        // `const ebdev = ...` has `=` not `:` after, should skip
        let source = r#"
const ebdev = require("ebdev");
toolchain: { ebdev: "0.2.0" }
"#;
        assert_eq!(extract_ebdev_version(source), Some("0.2.0".into()));
    }

    // ── Object form edge cases ──────────────────────────────────────────

    #[test]
    fn object_form_with_extra_fields() {
        assert_eq!(
            extract_ebdev_version(r#"ebdev: { version: "0.1.0", extra: "stuff" }"#),
            Some("0.1.0".into()),
        );
    }

    #[test]
    fn object_form_version_not_first_field() {
        assert_eq!(
            extract_ebdev_version(r#"ebdev: { extra: "stuff", version: "0.4.0" }"#),
            Some("0.4.0".into()),
        );
    }

    #[test]
    fn object_form_no_version_field() {
        assert_eq!(
            extract_ebdev_version(r#"ebdev: { name: "test" }"#),
            None,
        );
    }

    // ── Template literals and other non-quote values ────────────────────

    #[test]
    fn template_literal_not_matched() {
        assert_eq!(extract_ebdev_version("ebdev: `0.1.0`"), None);
    }

    #[test]
    fn numeric_value_not_matched() {
        assert_eq!(extract_ebdev_version("ebdev: 010"), None);
    }

    // ── Edge cases ──────────────────────────────────────────────────────

    #[test]
    fn version_at_end_of_file_no_newline() {
        assert_eq!(
            extract_ebdev_version(r#"ebdev: "0.9.0""#),
            Some("0.9.0".into()),
        );
    }

    #[test]
    fn only_ebdev_keyword_no_colon() {
        assert_eq!(extract_ebdev_version("ebdev"), None);
    }

    #[test]
    fn ebdev_at_very_end() {
        assert_eq!(extract_ebdev_version("foo ebdev"), None);
    }

    #[test]
    fn multiple_configs_returns_first() {
        let source = r#"
ebdev: "0.1.0",
ebdev: "0.2.0",
"#;
        assert_eq!(extract_ebdev_version(source), Some("0.1.0".into()));
    }

    #[test]
    fn newline_between_key_and_colon() {
        // Unlikely but technically: ebdev\n: "0.1.0"
        // trim_start() handles newlines
        let source = "ebdev\n  : \"0.1.0\"";
        assert_eq!(extract_ebdev_version(source), Some("0.1.0".into()));
    }

    // ── looks_like_version unit tests ───────────────────────────────────

    #[test]
    fn version_validator() {
        assert!(looks_like_version("0.1.0"));
        assert!(looks_like_version("1.0.0"));
        assert!(looks_like_version("12.34.56"));
        assert!(looks_like_version("0.1.0-beta.1"));
        assert!(looks_like_version("1.0.0-rc1"));

        assert!(!looks_like_version("hello"));
        assert!(!looks_like_version("0.1"));
        assert!(!looks_like_version("0.1."));
        assert!(!looks_like_version(".0.1.0"));
        assert!(!looks_like_version(""));
        assert!(!looks_like_version("0.1.0.0"));
    }
}
