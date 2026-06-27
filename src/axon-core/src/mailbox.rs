//! REQ-AXO-902112 / DEC-AXO-901663 — MAILBOX MVP core (MBX-1).
//!
//! Pure envelope + crypto for the inter-project LLM mailbox. The DB ops (send /
//! read / cursor) live in the MCP handler (`mcp/tools_mailbox.rs`); this module is
//! the A2A-aligned envelope construction, the deterministic dedup id, and the
//! HMAC-per-project signature. MVP integrity = a single server secret
//! (`AXON_MAILBOX_SECRET`) from which each project's token is derived; MBX-5
//! replaces the token source with a stored per-project keypair without touching
//! call sites.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

fn server_secret() -> Vec<u8> {
    std::env::var("AXON_MAILBOX_SECRET")
        .unwrap_or_else(|_| "axon-mailbox-dev-secret-v1".to_string())
        .into_bytes()
}

/// Per-project token derived from the single server secret (MVP). Swappable for a
/// stored keypair at MBX-5.
fn project_token(project: &str) -> Vec<u8> {
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

/// HMAC-SHA256 signature of a canonical envelope under the sender's project token.
pub fn sign(from_project: &str, canonical: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(&project_token(from_project)).expect("hmac key");
    mac.update(canonical.as_bytes());
    to_hex(&mac.finalize().into_bytes())
}

/// Constant-time verify (delegates to the hmac crate's `verify_slice`).
pub fn verify(from_project: &str, canonical: &str, sig: &str) -> bool {
    let mut mac = match HmacSha256::new_from_slice(&project_token(from_project)) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(canonical.as_bytes());
    match from_hex(sig) {
        Some(raw) => mac.verify_slice(&raw).is_ok(),
        None => false,
    }
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
    fn message_id_is_deterministic_and_dedup_aligned() {
        let a = message_id("NEX", "AXO", "idem-1");
        let b = message_id("NEX", "AXO", "idem-1");
        let c = message_id("NEX", "AXO", "idem-2");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("msg-"));
    }
}
