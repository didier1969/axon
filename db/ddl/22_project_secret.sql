-- REQ-AXO-902117 (MBX-5) — per-project signing secret + ACL scaffold (MECHANISM).
--
-- MBX-5 swaps the mailbox integrity token *source* from the single derived
-- server secret (AXON_MAILBOX_SECRET, see `crate::mailbox`) to a stored
-- per-project token, WITHOUT changing the HMAC scheme. This is the MECHANISM
-- only: the confidentiality / H1 / JWS policy stays GATED (deferred). When a
-- project has a row here, its outbound messages are signed under this token;
-- absent a row the writer falls back to the derived token, so every message
-- ever sent still verifies (the resolver tries the stored token first, then the
-- derived token for pre-provision rows — see `tools_mailbox::mailbox_verify`).
--
-- Runtime data (not SOLL intent) → `axon` schema, fully reconstructible
-- (rotating a token only invalidates signatures minted under the old one; the
-- append-only log is preserved).
CREATE SCHEMA IF NOT EXISTS axon;

-- MBX-5 — per-project signing token. `token` is opaque key material (32 random
-- bytes minted at first outbox_send, see `ensure_project_secret`). HMAC key, not
-- a keypair: the asymmetric JWS upgrade is the deferred POLICY, this table is the
-- swappable SOURCE the MBX-1 module-doc anticipated.
CREATE TABLE IF NOT EXISTS axon.project_secret (
    project_code TEXT        NOT NULL PRIMARY KEY,
    token        BYTEA       NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- MBX-5 — directional ACL scaffold (MECHANISM, default-OPEN). A row with
-- mode='deny' for (from_project → to_project) blocks that edge; the ABSENCE of a
-- deny row authorises (default-open). Whether a deny is ENFORCED (reject) or only
-- OBSERVED (logged, message still delivered) is gated by env
-- `AXON_MAILBOX_ACL_ENFORCE` (default 0 = observe-only). The POLICY — default
-- open vs closed, who-may-write-to-whom — stays operator-owned; this table + the
-- flag are the mechanism only.
CREATE TABLE IF NOT EXISTS axon.mailbox_acl (
    from_project TEXT        NOT NULL,
    to_project   TEXT        NOT NULL,
    mode         TEXT        NOT NULL DEFAULT 'allow',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (from_project, to_project)
);
