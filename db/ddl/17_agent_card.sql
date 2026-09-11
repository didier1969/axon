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

-- REQ-AXO-902118 (MBX-6) — Agent Cards: A2A capability discovery.
-- A project publishes its A2A AgentCard (well-known: /.well-known/agent-card.json);
-- a third party reads it + discovers peers by skill tag. Runtime data (not SOLL
-- intent) → `axon` schema, fully reconstructible. Owner-write only at the handler.
CREATE SCHEMA IF NOT EXISTS axon;

-- One card per project (the owner). `card` is the canonical A2A AgentCard JSON
-- { name, description, url, version, protocolVersion, capabilities{...},
--   defaultInputModes, defaultOutputModes, skills:[{id,name,description,tags}] };
-- the top-level columns are denormalised projections for indexing/listing.
CREATE TABLE IF NOT EXISTS axon.agent_card (
    project_code   TEXT        PRIMARY KEY,
    name           TEXT,
    description    TEXT        DEFAULT '',
    version        TEXT        DEFAULT '1.0.0',
    card           JSONB       NOT NULL,
    -- HMAC_SHA256(project_token[project_code], canonical_card(project, card)).
    -- Internal interop signature (MVP) — real A2A integrity = JWS (gap, see handler).
    sig            TEXT        DEFAULT '',
    schema_version INT         DEFAULT 1,
    updated_at     TIMESTAMPTZ DEFAULT now()
);

-- MBX-6 — discovery by skill tag. `list(skill=…)` filters cards whose
-- card->'skills' contains a skill carrying that tag; GIN over the skills array
-- serves the containment query.
SELECT public.create_index_if_absent('axon', 'agent_card_skills_idx', $idx$
    CREATE INDEX agent_card_skills_idx ON axon.agent_card USING gin ((card -> 'skills'))
$idx$);
