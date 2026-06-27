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
CREATE INDEX IF NOT EXISTS agent_card_skills_idx
    ON axon.agent_card USING gin ((card -> 'skills'));
