-- REQ-AXO-902328 — ce fichier est appliqué par le brain à CHAQUE boot.
-- Il ne l'était PAS avant le 2026-08-25 : il manquait à la liste `include_str!`
-- écrite à la main de `postgres/ddl.rs` (9 fichiers sur 25 absents), donc il
-- n'avait jamais reçu la discipline de REQ-AXO-902339 — « ne pas réclamer un
-- verrou avant de tester si l'on a quelque chose à faire ». `ADD COLUMN IF NOT
-- EXISTS`, `CREATE INDEX IF NOT EXISTS`, `DROP INDEX/TRIGGER IF EXISTS` prennent
-- tous leur verrou AVANT le test d'existence : sur `axon.practice` et
-- `axon.mailbox_message`, écrites en continu, c'est une famine, pas une course.
-- Les `ADD COLUMN` de ce fichier passent désormais par `add_column_if_absent`.
--
-- ⚠️ Les `CREATE INDEX IF NOT EXISTS` NE sont PAS convertis, et c'est délibéré :
-- les 16 fichiers appliqués au boot depuis toujours en portent 26 de la même
-- forme, sans incident mesuré. Les convertir ici seulement donnerait DEUX
-- disciplines pour une seule classe d'énoncé — exactement la divergence que
-- REQ-AXO-902328 ferme. La classe entière (45 CREATE INDEX + 3 DROP nus sur les
-- 25 fichiers) est logée en REQ, à traiter d'un bloc ou pas du tout.

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
