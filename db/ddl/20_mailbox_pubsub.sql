-- REQ-AXO-902328 — ce fichier est appliqué par le brain à CHAQUE boot.
-- Il ne l'était PAS avant le 2026-08-25 : il manquait à la liste `include_str!`
-- écrite à la main de `postgres/ddl.rs` (9 fichiers sur 25 absents), donc il
-- n'avait jamais reçu la discipline de REQ-AXO-902339 — « ne pas réclamer un
-- verrou avant de tester si l'on a quelque chose à faire ». `ADD COLUMN IF NOT
-- EXISTS`, `CREATE INDEX IF NOT EXISTS`, `DROP INDEX/TRIGGER IF EXISTS` prennent
-- tous leur verrou AVANT le test d'existence : sur `axon.practice` et
-- `axon.mailbox_message`, écrites en continu, c'est une famine, pas une course.
-- Les `ADD COLUMN` de ce fichier passent désormais par `add_column_if_absent`.
-- REQ-AXO-902475 — l'ensemble des `CREATE INDEX IF NOT EXISTS` et `DROP` sur les
-- 25 fichiers passe désormais par les gardes catalogue lock-free
-- `create_index_if_absent`, `drop_index_if_present`, `drop_trigger_if_present`.

-- REQ-AXO-902119 (MBX-7) — MAILBOX pub/sub + broadcast/multicast + rooms.
-- Decouples the emitter from N subscribers (topics), supports broadcast decisions
-- (fan-out to every registered project via '*'), and multi-party rooms. Runtime
-- data (not SOLL intent) → `axon` schema, fully reconstructible. Applied to live by
-- the canonical DDL loop (scripts/lib/ensure-runtime.sh apply_canonical_ddl /
-- promote) and baked into every test clone (apply_sql_dir) — both read this dir
-- lexically, so no include_str! registration is required.
--
-- FAN-OUT MODEL (handler tools_mailbox_pubsub.rs): mcp_outbox_send resolves the
-- recipient set AT SEND (subscribers / room members / ProjectCodeRegistry for '*')
-- and INSERTs one MATERIALISED axon.mailbox_message row per recipient — stamped
-- with `topic` / `room_id`. Because each row carries a concrete `to_project`, the
-- existing inbox_read / inbox_unread / per-recipient cursor / LISTEN-NOTIFY path is
-- reused verbatim. The shared `context_id` groups the whole broadcast as one thread.

CREATE SCHEMA IF NOT EXISTS axon;

-- MBX-7 — topics. A topic is a named pub/sub channel; subscribers receive every
-- message published to it. `created_by` is the project that first declared it.
CREATE TABLE IF NOT EXISTS axon.mailbox_topic (
    topic       TEXT        NOT NULL PRIMARY KEY,
    created_by  TEXT        NOT NULL DEFAULT '',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- MBX-7 — subscriptions. (topic, project_code) is the fan-out edge: when a message
-- is published to `topic`, one materialised row is delivered to each `project_code`
-- here. PK dedups a double-subscribe.
CREATE TABLE IF NOT EXISTS axon.mailbox_subscription (
    topic         TEXT        NOT NULL,
    project_code  TEXT        NOT NULL,
    subscribed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (topic, project_code)
);

-- Fan-out lookup: resolve all subscribers of a topic at send time.
SELECT public.create_index_if_absent('axon', 'mailbox_subscription_topic_idx', $idx$
    CREATE INDEX mailbox_subscription_topic_idx ON axon.mailbox_subscription (topic)
$idx$);

-- MBX-7 — rooms (multi-party). A room groups N projects; a message addressed
-- `to_room` is delivered to every member. `created_by` is the room owner.
CREATE TABLE IF NOT EXISTS axon.mailbox_room (
    room_id     TEXT        NOT NULL PRIMARY KEY,
    created_by  TEXT        NOT NULL DEFAULT '',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS axon.mailbox_room_member (
    room_id       TEXT        NOT NULL,
    project_code  TEXT        NOT NULL,
    joined_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (room_id, project_code)
);

-- Fan-out lookup: resolve all members of a room at send time.
SELECT public.create_index_if_absent('axon', 'mailbox_room_member_room_idx', $idx$
    CREATE INDEX mailbox_room_member_room_idx ON axon.mailbox_room_member (room_id)
$idx$);

-- Materialised fan-out provenance: every delivered broadcast/multicast row records
-- the topic / room it was stamped from (NULL for a point-to-point send). IF NOT
-- EXISTS so re-apply over the MVP store is a no-op.
SELECT public.add_column_if_absent('axon', 'mailbox_message', 'topic',   'TEXT');
SELECT public.add_column_if_absent('axon', 'mailbox_message', 'room_id', 'TEXT');

-- CRITICAL (dedup vs. fan-out) — the MVP UNIQUE(from_project, idempotency_key)
-- rejects rows 2..N of a single broadcast (same sender + key, different recipient).
-- Widen the dedup key to include the recipient so point-to-point idempotency is
-- preserved while fan-out can materialise one row per recipient under one key.
DO $do$
BEGIN
    -- Drop legacy 2-column index if present (REQ-AXO-902475 lock-free migration)
    IF EXISTS (
        SELECT 1 FROM pg_index i
        JOIN pg_class c ON c.oid = i.indexrelid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'axon' AND c.relname = 'mailbox_message_idem_idx' AND i.indnatts = 2
    ) THEN
        PERFORM public.drop_index_if_present('axon', 'mailbox_message_idem_idx');
    END IF;
END
$do$;
SELECT public.create_index_if_absent('axon', 'mailbox_message_idem_idx', $idx$
    CREATE UNIQUE INDEX mailbox_message_idem_idx
        ON axon.mailbox_message (from_project, to_project, idempotency_key)
$idx$);
