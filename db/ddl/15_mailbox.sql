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

-- REQ-AXO-902112 (umbrella) / DEC-AXO-901663 — MAILBOX MVP store.
-- Inter-project asynchronous LLM mailbox: Axon is the central exchange. A2A
-- v1.0-aligned envelope, HMAC-per-project integrity, event-sourced append-only log
-- (PIL-AXO-9004), at-least-once + idempotent dedup, per-recipient read cursor,
-- LISTEN/NOTIFY signal on arrival (MBX-3). Runtime data (not SOLL intent) →
-- `axon` schema, fully reconstructible.
CREATE SCHEMA IF NOT EXISTS axon;

-- MBX-1 — the message log. The canonical A2A envelope lives in `envelope` (JSONB);
-- the addressing/dedup/thread fields are denormalised as columns for indexing.
CREATE TABLE IF NOT EXISTS axon.mailbox_message (
    id              BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    -- A2A messageId (server-assigned, stable, unique). Used by in_reply_to.
    message_id      TEXT        NOT NULL,
    -- A2A contextId = conversation/thread id (groups a multi-turn exchange).
    context_id      TEXT        NOT NULL DEFAULT '',
    from_project    TEXT        NOT NULL,
    to_project      TEXT        NOT NULL,
    -- A2A role/kind (default agent message).
    role            TEXT        NOT NULL DEFAULT 'agent',
    kind            TEXT        NOT NULL DEFAULT 'message',
    subject         TEXT        NOT NULL DEFAULT '',
    -- Dense, pointer-bearing body (umbrella principle: stigmergy, point at SOLL
    -- ids / symbols / artefact hashes rather than inlining recoverable content).
    body_dense      TEXT        NOT NULL DEFAULT '',
    -- Full A2A-aligned envelope: { messageId, contextId, role, kind, from, to,
    -- parts:[{kind:data, data:{subject, body_dense, ref_soll_ids}}], inReplyTo,
    -- idempotencyKey, ts }. Canonical wire shape; columns above are projections.
    envelope        JSONB       NOT NULL,
    -- Sender-scoped dedup key (at-least-once + idempotent): see UNIQUE below.
    idempotency_key TEXT        NOT NULL,
    in_reply_to     TEXT,
    priority        TEXT        NOT NULL DEFAULT 'normal',
    schema_version  INTEGER     NOT NULL DEFAULT 1,
    -- HMAC_SHA256(project_token[from_project], canonical(envelope without sig)).
    sig             TEXT        NOT NULL DEFAULT '',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Optional retention horizon (TTL / dead-letter sweep); NULL = keep.
    ttl_at          TIMESTAMPTZ,
    notified_at     TIMESTAMPTZ,
    read_at         TIMESTAMPTZ,
    acknowledged_at TIMESTAMPTZ
);

-- REQ-AXO-902548 — quadri-état de traçabilité mailbox (notified_at, read_at, acknowledged_at)
SELECT public.add_column_if_absent('axon', 'mailbox_message', 'notified_at',     'TIMESTAMPTZ');
SELECT public.add_column_if_absent('axon', 'mailbox_message', 'read_at',         'TIMESTAMPTZ');
SELECT public.add_column_if_absent('axon', 'mailbox_message', 'acknowledged_at', 'TIMESTAMPTZ');

-- Idempotent dedup: a re-sent message (same sender + key + recipient) is a no-op
-- (ON CONFLICT DO NOTHING at the writer). Anchors at-least-once delivery.
--
-- REQ-AXO-902576 — la cle porte le DESTINATAIRE. La version d'origine
-- UNIQUE(from_project, idempotency_key) est IMPOSSIBLE depuis le fan-out (MBX-7) :
-- un broadcast materialise une ligne par destinataire sous la meme cle, et la base
-- en compte 75 par cle. `CREATE UNIQUE INDEX` echouait donc en 23505, ce qui fait
-- echouer TOUT le bootstrap du schema — « Fatal Error initializing GraphStore » —
-- et tuait l'indexeur au demarrage.
-- 20_mailbox_pubsub.sql corrigeait deja la definition, mais TROP TARD : le fichier
-- 15 s'applique avant et meurt. Les deux fichiers declarent desormais la MEME
-- definition, donc l'ordre d'application cesse d'importer.
SELECT public.create_index_if_absent('axon', 'mailbox_message_idem_idx', $idx$
    CREATE UNIQUE INDEX mailbox_message_idem_idx
        ON axon.mailbox_message (from_project, to_project, idempotency_key)
$idx$);

-- inbox_read(to=project, unread|since): scan the recipient's messages by id.
SELECT public.create_index_if_absent('axon', 'mailbox_message_inbox_idx', $idx$
    CREATE INDEX mailbox_message_inbox_idx ON axon.mailbox_message (to_project, id)
$idx$);

-- MBX-4 — thread retrieval (conversation_id) + FTS over subject+body for
-- searchable threads. The btree serves exact-thread fetch; the GIN index serves
-- `inbox_read(search=…)` full-text queries.
SELECT public.create_index_if_absent('axon', 'mailbox_message_thread_idx', $idx$
    CREATE INDEX mailbox_message_thread_idx ON axon.mailbox_message (context_id, id)
$idx$);
SELECT public.create_index_if_absent('axon', 'mailbox_message_fts_idx', $idx$
    CREATE INDEX mailbox_message_fts_idx ON axon.mailbox_message USING gin (to_tsvector('simple', subject || ' ' || body_dense))
$idx$);

-- MBX-2 — per-recipient read cursor. `unread` = messages to=project with
-- id > last_read_id. Advanced (monotonically) when the recipient reads.
CREATE TABLE IF NOT EXISTS axon.mailbox_cursor (
    project_code TEXT        NOT NULL PRIMARY KEY,
    last_read_id BIGINT      NOT NULL DEFAULT 0,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- MBX-3 / REQ-AXO-902548 — signal on arrival & wake. Notify the recipient's channel
-- so active sessions wake up without polling (<5s p95). Payload is signature-only
-- metadata: { to, from, message_id, context_id, id, priority }.
CREATE OR REPLACE FUNCTION axon.mailbox_notify() RETURNS trigger AS $$
DECLARE
    payload text;
BEGIN
    payload := json_build_object(
        'to', NEW.to_project,
        'from', NEW.from_project,
        'message_id', NEW.message_id,
        'context_id', NEW.context_id,
        'id', NEW.id,
        'priority', NEW.priority
    )::text;
    PERFORM pg_notify('axon_mailbox', payload);
    PERFORM pg_notify('axon_mailbox_wake', payload);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

SELECT public.create_trigger_if_absent(
    'axon', 'mailbox_message', 'mailbox_message_notify', $trg$
    CREATE TRIGGER mailbox_message_notify
        AFTER INSERT ON axon.mailbox_message
        FOR EACH ROW EXECUTE FUNCTION axon.mailbox_notify()
$trg$);
