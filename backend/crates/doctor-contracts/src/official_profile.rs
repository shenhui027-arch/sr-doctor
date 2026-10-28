//! Official interpretation is a separate text artifact, never a raw Profile input.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use crate::{ConnectedQueryRef, VersionEvidence};

pub fn official_profile_text_hash(text: &str) -> String {
    utf8_sha256(text)
}

/// SHA-256 of the distinct, additive display projection. This never replaces
/// [`official_profile_text_hash`], which remains bound to the returned source
/// artifact byte-for-byte.
pub fn official_profile_display_hash(display_text: &str) -> String {
    utf8_sha256(display_text)
}

fn utf8_sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// Create the fixed v1 display projection without interpreting profile
/// contents. Complete ANSI SGR sequences are omitted; every remaining control
/// character except LF and TAB is rendered as a visible `\\u{XXXX}` escape.
/// The source artifact, its ordering, Unicode text, whitespace and units are
/// otherwise preserved exactly.
pub fn official_profile_display_text(text: &str) -> String {
    let mut display = String::with_capacity(text.len());
    let mut offset = 0;

    while offset < text.len() {
        let remaining = &text[offset..];
        if let Some(sgr_len) = complete_ansi_sgr_len(remaining) {
            offset += sgr_len;
            continue;
        }

        let character = remaining
            .chars()
            .next()
            .expect("remaining text is non-empty while offset is in bounds");
        match character {
            '\n' | '\t' => display.push(character),
            _ if character.is_control() => {
                use std::fmt::Write as _;

                write!(display, r"\u{{{:04X}}}", character as u32)
                    .expect("writing a String cannot fail");
            }
            _ => display.push(character),
        }
        offset += character.len_utf8();
    }

    display
}

/// Return the byte length only for one complete ANSI SGR sequence:
/// `ESC [` followed by ASCII digits, `;` or `:`, then `m`. Unknown, malformed
/// and truncated terminal sequences deliberately return `None`, so the outer
/// scan retains every following ordinary character.
fn complete_ansi_sgr_len(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    if !bytes.starts_with(b"\x1b[") {
        return None;
    }

    let mut offset = 2;
    while let Some(byte) = bytes.get(offset) {
        match byte {
            b'0'..=b'9' | b';' | b':' => offset += 1,
            b'm' => return Some(offset + 1),
            _ => return None,
        }
    }
    None
}

/// The sole server-verified authority for a fixed official Profile read.
/// Discovery and standard execution are intentionally disjoint: an execution
/// attempt never acquires a synthetic discovery generation or observed row.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "kind", rename_all = "snake_case")]
pub enum OfficialProfileIdentity {
    Discovery { discovery_generation: u32 },
    VerifiedStandardAttempt { execution_attempt_id: String },
}

/// The only execution method that can issue an official Profile-read receipt.
/// A standard Profile capture is the only execution method; this receipt
/// binds that one attempt to a later official read.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum OfficialProfileEligibilityMethod {
    StandardProfile,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct OfficialProfileReadRequest {
    pub connection_id: String,
    pub query_id: String,
    pub identity: OfficialProfileIdentity,
    /// Empty means the whole official tree. The connector validates count,
    /// signed-integer range and uniqueness before any remote operation.
    pub node_ids: Vec<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum OfficialProfileArtifactKind {
    AnalyzeProfileText,
}

/// The fixed display projection contract for [`OfficialProfileReadResponse`].
/// `ansi_sgr_strip_controls_escape_v1` omits only complete ANSI SGR sequences
/// and renders remaining control characters (including CR) visibly, while
/// retaining LF, TAB and all non-control Unicode text.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum OfficialProfileDisplayNormalization {
    AnsiSgrStripControlsEscapeV1,
}

/// A successful read only. Unavailable reads use the connected typed error
/// envelope, not an empty artifact or an inferred replacement measurement.
#[derive(Clone, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct OfficialProfileReadResponse {
    pub kind: OfficialProfileArtifactKind,
    pub query: ConnectedQueryRef,
    /// Echoes the exact authority the server accepted for this fixed read.
    pub identity: OfficialProfileIdentity,
    pub detected_version: Option<VersionEvidence>,
    pub node_ids: Vec<u32>,
    /// Completion time of this read, not the query/snapshot observation time.
    pub read_at_utc: String,
    /// None unless the source itself provides a reliable observation time.
    pub profile_observed_at_utc: Option<String>,
    /// SHA-256 of the returned UTF-8 text, independent of analysis report keys.
    pub source_hash: String,
    /// Byte-for-byte source artifact; it may contain ANSI or other control
    /// sequences as well as SQL or table names. Default readable rendering uses
    /// the `display_text` field as a text node; this field remains available for
    /// explicit source inspection and is never HTML or terminal instruction
    /// input.
    pub text: String,
    /// Additive, non-executing readable projection of `text`; never a source
    /// replacement and never a metric, SQL or HTML interpretation.
    pub display_text: String,
    /// SHA-256 of `display_text` UTF-8 bytes, distinct from `source_hash`.
    pub display_hash: String,
    /// Fixed normalization version used to derive `display_text`.
    pub display_normalization: OfficialProfileDisplayNormalization,
    pub source_refs: Vec<String>,
    pub limitations: Vec<String>,
}

impl std::fmt::Debug for OfficialProfileReadResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OfficialProfileReadResponse { redacted: true }")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> serde_json::Value {
        json!({"connection_id":"session", "query_id":"00000000-0000-0000-0000-000000000001",
            "identity":{"kind":"discovery", "discovery_generation":2}, "node_ids":[0,27]})
    }

    #[test]
    fn text_hash_uses_utf8_bytes_not_json_serialization() {
        assert_eq!(
            official_profile_text_hash(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_ne!(
            official_profile_text_hash("a\nb"),
            official_profile_text_hash("ab")
        );
        assert_ne!(
            official_profile_text_hash("节点"),
            official_profile_text_hash("\"节点\"")
        );
    }

    #[test]
    fn display_projection_removes_only_complete_sgr_and_keeps_source_hash_bytes() {
        let source = "\u{1b}[38;2;250;128;114m节点 27\u{1b}[0m\r\n\t5µs";
        let display = official_profile_display_text(source);

        assert_eq!(display, "节点 27\\u{000D}\n\t5µs");
        assert_eq!(
            official_profile_text_hash(source),
            "37c521020a8873fa9b7636d2e3ad7cee04d4d82c34f41d5e62e2e29fd8a02f40"
        );
        assert_eq!(
            official_profile_display_hash(&display),
            "9ded1c09fe57f0ff4b471ddcce9ba8c943ef2beeba1658915e4729428d7a03d5"
        );
        assert_ne!(
            official_profile_text_hash(source),
            official_profile_display_hash(&display)
        );
    }

    #[test]
    fn display_projection_keeps_unknown_and_truncated_terminal_text_visible() {
        let source = "\u{1b}[2JX\u{1b}]0;title\u{7}Y\u{1b}[31";

        assert_eq!(
            official_profile_display_text(source),
            r"\u{001B}[2JX\u{001B}]0;title\u{0007}Y\u{001B}[31"
        );
    }

    #[test]
    fn display_projection_escapes_all_controls_except_lf_and_tab() {
        assert_eq!(
            official_profile_display_text("A\0\u{7f}\u{85}\r\n\tB"),
            "A\\u{0000}\\u{007F}\\u{0085}\\u{000D}\n\tB"
        );
    }

    #[test]
    fn display_projection_preserves_html_unicode_empty_and_layout_text() {
        assert_eq!(
            official_profile_display_text("<script>节点 27</script>\n  A\tB"),
            "<script>节点 27</script>\n  A\tB"
        );
        assert_eq!(official_profile_display_text(""), "");
    }

    #[test]
    fn display_projection_is_idempotent_and_normalization_wire_value_is_fixed() {
        let first = official_profile_display_text("\u{1b}[mA\r\u{1b}[38:2::255:255:255mB");
        assert_eq!(official_profile_display_text(&first), first);
        assert_eq!(
            serde_json::to_value(OfficialProfileDisplayNormalization::AnsiSgrStripControlsEscapeV1)
                .unwrap(),
            json!("ansi_sgr_strip_controls_escape_v1")
        );
    }

    #[test]
    fn request_round_trip_and_empty_filter_are_explicit() {
        let value = request();
        let decoded: OfficialProfileReadRequest = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
        let mut empty = value;
        empty["node_ids"] = json!([]);
        assert!(serde_json::from_value::<OfficialProfileReadRequest>(empty).is_ok());
    }

    #[test]
    fn identity_and_filter_fields_are_required() {
        for field in ["connection_id", "query_id", "identity", "node_ids"] {
            let mut value = request();
            value.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<OfficialProfileReadRequest>(value).is_err());
        }
    }

    #[test]
    fn cannot_accept_sql_or_non_integer_node_filters() {
        let mut value = request();
        value["candidate_sql"] = json!("SELECT 1");
        assert!(serde_json::from_value::<OfficialProfileReadRequest>(value).is_err());
        for filter in [json!([-1]), json!([1.5]), json!(["27"]), json!(null)] {
            let mut value = request();
            value["node_ids"] = filter;
            assert!(serde_json::from_value::<OfficialProfileReadRequest>(value).is_err());
        }
    }

    #[test]
    fn identities_are_strictly_tagged_and_mutually_exclusive() {
        let discovery = request();
        assert!(serde_json::from_value::<OfficialProfileReadRequest>(discovery).is_ok());

        let verified = json!({
            "connection_id":"session",
            "query_id":"00000000-0000-0000-0000-000000000001",
            "identity":{"kind":"verified_standard_attempt", "execution_attempt_id":"00000000-0000-0000-0000-000000000002"},
            "node_ids":[],
        });
        assert!(serde_json::from_value::<OfficialProfileReadRequest>(verified).is_ok());

        for identity in [
            json!({"kind":"discovery", "discovery_generation":2, "execution_attempt_id":"00000000-0000-0000-0000-000000000002"}),
            json!({"kind":"verified_standard_attempt", "execution_attempt_id":"00000000-0000-0000-0000-000000000002", "discovery_generation":2}),
            json!({"kind":"unknown", "discovery_generation":2}),
            json!({"discovery_generation":2}),
        ] {
            let mut value = request();
            value["identity"] = identity;
            assert!(serde_json::from_value::<OfficialProfileReadRequest>(value).is_err());
        }
    }

    #[test]
    fn eligibility_method_is_standard_profile_only() {
        assert_eq!(
            serde_json::to_value(OfficialProfileEligibilityMethod::StandardProfile).unwrap(),
            json!("standard_profile")
        );
        assert!(
            serde_json::from_value::<OfficialProfileEligibilityMethod>(json!(
                "native_explain_analyze"
            ))
            .is_err()
        );
    }

    #[test]
    fn artifact_kind_does_not_accept_raw_profile_or_execution() {
        assert_eq!(
            serde_json::to_value(OfficialProfileArtifactKind::AnalyzeProfileText).unwrap(),
            json!("analyze_profile_text")
        );
        for kind in ["raw_profile", "explain_analyze", "runtime_profile"] {
            assert!(serde_json::from_value::<OfficialProfileArtifactKind>(json!(kind)).is_err());
        }
    }
}
