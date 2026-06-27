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
CREATE INDEX IF NOT EXISTS mailbox_subscription_topic_idx
    ON axon.mailbox_subscription (topic);

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
CREATE INDEX IF NOT EXISTS mailbox_room_member_room_idx
    ON axon.mailbox_room_member (room_id);

-- Materialised fan-out provenance: every delivered broadcast/multicast row records
-- the topic / room it was stamped from (NULL for a point-to-point send). IF NOT
-- EXISTS so re-apply over the MVP store is a no-op.
ALTER TABLE axon.mailbox_message
    ADD COLUMN IF NOT EXISTS topic   TEXT,
    ADD COLUMN IF NOT EXISTS room_id TEXT;

-- CRITICAL (dedup vs. fan-out) — the MVP UNIQUE(from_project, idempotency_key)
-- rejects rows 2..N of a single broadcast (same sender + key, different recipient).
-- Widen the dedup key to include the recipient so point-to-point idempotency is
-- preserved while fan-out can materialise one row per recipient under one key.
DROP INDEX IF EXISTS axon.mailbox_message_idem_idx;
CREATE UNIQUE INDEX IF NOT EXISTS mailbox_message_idem_idx
    ON axon.mailbox_message (from_project, to_project, idempotency_key);
