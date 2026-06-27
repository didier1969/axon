//! REQ-AXO-902112 / DEC-AXO-901663 — MAILBOX MVP core (MBX-1).
//!
//! Pure envelope + crypto for the inter-project LLM mailbox. The DB ops (send /
//! read / cursor) live in the MCP handler (`mcp/tools_mailbox.rs`); this module is
//! the A2A-aligned envelope construction, the deterministic dedup id, and the
//! HMAC-per-project signature. MVP integrity = a single server secret
//! (`AXON_MAILBOX_SECRET`) from which each project's token is derived; MBX-5
//! replaces the token *source* with a stored per-project token without touching
//! call sites.
//!
//! REQ-AXO-902117 (MBX-5) — the MECHANISM is here as the token-aware
//! [`sign_with_token`] / [`verify_with_token`] pair: this module stays PURE (no
//! DB/`&self`), and the per-project token is resolved DB-side in
//! `mcp/tools_mailbox.rs` (stored `axon.project_secret` token, falling back to
//! [`derived_project_token`] when absent). The HMAC scheme is unchanged — the
//! confidentiality / H1 / JWS upgrade is the deferred POLICY, not this slice.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

fn server_secret() -> Vec<u8> {
    std::env::var("AXON_MAILBOX_SECRET")
        .unwrap_or_else(|_| "axon-mailbox-dev-secret-v1".to_string())
        .into_bytes()
}

/// Per-project token derived from the single server secret (MVP fallback). MBX-5
/// (REQ-AXO-902117) prefers a stored per-project token from `axon.project_secret`
/// (resolved DB-side in `tools_mailbox`); this derivation is the retro-compatible
/// fallback for any project without a stored token, so every message ever signed
/// still verifies.
pub fn derived_project_token(project: &str) -> Vec<u8> {
    let mut mac =
        HmacSha256::new_from_slice(&server_secret()).expect("hmac accepts any key length");
    mac.update(project.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Decode a lowercase/uppercase hex string into bytes (`None` on odd length or
/// any non-hex digit). Public so the MBX-5 token resolver in `tools_mailbox` can
/// decode the `encode(token,'hex')` projection of the `BYTEA` stored secret.
pub fn decode_hex(s: &str) -> Option<Vec<u8>> {
    from_hex(s)
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Deterministic canonical string over the stable envelope fields (excludes sig).
/// Sign and verify build it identically, so a re-serialisation can never change
/// the bytes that were signed.
#[allow(clippy::too_many_arguments)]
pub fn canonical(
    from: &str,
    to: &str,
    context_id: &str,
    message_id: &str,
    kind: &str,
    idempotency_key: &str,
    in_reply_to: &str,
    subject: &str,
    body_dense: &str,
) -> String {
    format!(
        "v1|{from}|{to}|{context_id}|{message_id}|{kind}|{idempotency_key}|{in_reply_to}|{subject}|{body_dense}"
    )
}

/// REQ-AXO-902118 (MBX-6) — deterministic canonical string over a project's A2A
/// AgentCard, for sign/verify. JSON object key order is unstable across
/// serialisers, so we re-serialise the card through a recursively key-sorted
/// representation (objects → BTreeMap) and prefix the owner project. Sign and
/// verify build it identically, so a re-serialisation can never change the bytes
/// that were signed.
pub fn canonical_card(project: &str, card: &serde_json::Value) -> String {
    format!("card-v1|{project}|{}", canonicalize_json(card))
}

/// Recursive deterministic JSON serialisation: object keys sorted (BTreeMap),
/// arrays order-preserved, scalars verbatim. No whitespace.
fn canonicalize_json(v: &serde_json::Value) -> String {
    use std::collections::BTreeMap;
    match v {
        serde_json::Value::Object(map) => {
            let sorted: BTreeMap<&String, &serde_json::Value> = map.iter().collect();
            let inner: Vec<String> = sorted
                .iter()
                .map(|(k, val)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_default(),
                        canonicalize_json(val)
                    )
                })
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        serde_json::Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(canonicalize_json).collect();
            format!("[{}]", inner.join(","))
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// HMAC-SHA256 signature of a canonical envelope under an EXPLICIT token (MBX-5
/// mechanism). The caller resolves the token (stored per-project secret or the
/// derived fallback); this keeps the module pure (no DB).
pub fn sign_with_token(token: &[u8], canonical: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(token).expect("hmac accepts any key length");
    mac.update(canonical.as_bytes());
    to_hex(&mac.finalize().into_bytes())
}

/// Constant-time verify under an EXPLICIT token (MBX-5 mechanism; delegates to the
/// hmac crate's `verify_slice`).
pub fn verify_with_token(token: &[u8], canonical: &str, sig: &str) -> bool {
    let mut mac = match HmacSha256::new_from_slice(token) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(canonical.as_bytes());
    match from_hex(sig) {
        Some(raw) => mac.verify_slice(&raw).is_ok(),
        None => false,
    }
}

/// HMAC-SHA256 signature under the sender's DERIVED project token (MVP / fallback
/// path). Equivalent to `sign_with_token(&derived_project_token(from), …)`.
pub fn sign(from_project: &str, canonical: &str) -> String {
    sign_with_token(&derived_project_token(from_project), canonical)
}

/// Constant-time verify under the sender's DERIVED project token (MVP / fallback).
pub fn verify(from_project: &str, canonical: &str, sig: &str) -> bool {
    verify_with_token(&derived_project_token(from_project), canonical, sig)
}

/// Stable, dedup-aligned message id: the same (from, to, idempotency_key) yields
/// the same id, so a re-send is a true no-op. Non-cryptographic id — integrity is
/// the signature's job.
pub fn message_id(from: &str, to: &str, idempotency_key: &str) -> String {
    let mut h = Sha256::new();
    h.update(from.as_bytes());
    h.update(b"|");
    h.update(to.as_bytes());
    h.update(b"|");
    h.update(idempotency_key.as_bytes());
    format!("msg-{}", &to_hex(&h.finalize())[..24])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_round_trip() {
        let c = canonical("NEX", "AXO", "ctx-1", "msg-abc", "message", "idem-1", "", "subj", "body");
        let sig = sign("NEX", &c);
        assert!(verify("NEX", &c, &sig));
    }

    #[test]
    fn tamper_or_wrong_sender_fails() {
        let c = canonical("NEX", "AXO", "ctx-1", "msg-abc", "message", "idem-1", "", "subj", "body");
        let sig = sign("NEX", &c);
        // tampered body
        let c2 = canonical("NEX", "AXO", "ctx-1", "msg-abc", "message", "idem-1", "", "subj", "EVIL");
        assert!(!verify("NEX", &c2, &sig));
        // wrong sender (different derived token)
        assert!(!verify("AXO", &c, &sig));
        // garbage sig
        assert!(!verify("NEX", &c, "not-hex"));
    }

    #[test]
    fn canonical_card_is_key_order_independent() {
        // Same logical card, different key insertion order → identical canonical.
        let a = serde_json::json!({ "name": "AXO", "version": "1.0.0", "skills": [{ "id": "s1", "tags": ["a", "b"] }] });
        let b = serde_json::json!({ "skills": [{ "tags": ["a", "b"], "id": "s1" }], "version": "1.0.0", "name": "AXO" });
        assert_eq!(canonical_card("AXO", &a), canonical_card("AXO", &b));
        // Array order IS significant (skills order is meaningful).
        let c = serde_json::json!({ "name": "AXO", "version": "1.0.0", "skills": [{ "id": "s1", "tags": ["b", "a"] }] });
        assert_ne!(canonical_card("AXO", &a), canonical_card("AXO", &c));
    }

    #[test]
    fn canonical_card_sign_verify_round_trip() {
        let card = serde_json::json!({ "name": "AXO", "skills": [{ "id": "discover", "tags": ["a2a"] }] });
        let c = canonical_card("AXO", &card);
        let sig = sign("AXO", &c);
        assert!(verify("AXO", &c, &sig));
        // wrong owner token fails
        assert!(!verify("NEX", &c, &sig));
    }

    #[test]
    fn mbx5_stored_token_round_trip_and_distinct_from_derived() {
        // REQ-AXO-902117 (MBX-5) — a stored per-project token signs and verifies
        // independently of the derived fallback, and the two are NOT interchangeable.
        let c = canonical("NEX", "AXO", "ctx-1", "msg-abc", "message", "idem-1", "", "subj", "body");
        let stored = decode_hex("00112233445566778899aabbccddeeff").expect("valid hex");
        let sig = sign_with_token(&stored, &c);
        // round-trip under the same stored token
        assert!(verify_with_token(&stored, &c, &sig));
        // the derived token must NOT verify a stored-token signature (mechanism swap)
        assert!(!verify(&"NEX", &c, &sig));
        let derived = derived_project_token("NEX");
        assert!(!verify_with_token(&derived, &c, &sig));
        // and the legacy derived-sign path stays self-consistent (retro-compat)
        let dsig = sign("NEX", &c);
        assert!(verify("NEX", &c, &dsig));
        assert!(verify_with_token(&derived, &c, &dsig));
        assert_ne!(sig, dsig);
    }

    #[test]
    fn message_id_is_deterministic_and_dedup_aligned() {
        let a = message_id("NEX", "AXO", "idem-1");
        let b = message_id("NEX", "AXO", "idem-1");
        let c = message_id("NEX", "AXO", "idem-2");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("msg-"));
    }
}
