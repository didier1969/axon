-- REQ-AXO-902088 (S1 store canonique) + REQ-AXO-902093 (S6 réconciliation) —
-- persistance du squelette ContractNode (REQ-AXO-902087, modèle A : tables soll.*
-- dédiées, ADOSSÉES à la machinerie SOLL — PAS une duplication de soll.Node).
--
-- Le cœur logique (contract.rs : ContractNode + shape_hash + validate ; adequacy /
-- seal / binding / certification / expand) vit déjà en RAM. Cette couche lui donne
-- une racine durable :
--   • soll.Contract      = la forme DÉSIRÉE (kind + signature + post-conditions) +
--                          son sceau structurel Merkle (DEC-AXO-901657) + la
--                          baseline IST-OBSERVÉE pour la réconciliation S6.
--   • soll.ContractEdge  = les arêtes typées : CHILD (enfants Merkle, sealing
--                          partiel DEC-AXO-901659), SOLVES/EXPLAINS (cross-ref vers
--                          soll.Node — le pourquoi gouvernant) et REALIZED_BY
--                          (cross-ref vers ist.Symbol — l'ancre d'identité, repli
--                          rename-tracking DEC-AXO-901656, JAMAIS la certification).
--
-- Modèle de versioning du sceau : colonne `seal_revision` (millis) plutôt que de
-- forcer une ligne soll.Revision par re-sceau (le sceau structurel est déterministe
-- et content-addressed ; un horodatage monotone suffit à ordonner les re-sceaux sans
-- contaminer le journal d'audit d'intention).
--
-- DEFINE-ONCE / idempotent (CREATE … IF NOT EXISTS), appliqué en ordre lexical par
-- scripts/lib/ensure-runtime.sh:apply_canonical_ddl et par le template de test
-- (test_db.rs:apply_sql_dir), exactement comme 14..20_*.sql. PG replie les
-- identifiants non-quotés en minuscules (soll.Contract → soll.contract), cohérent
-- avec soll.Node / soll.Edge.

CREATE SCHEMA IF NOT EXISTS soll;

-- ── La forme désirée + son sceau ──────────────────────────────────────
-- Une ligne = un ContractNode canonique. `shape_hash` est le hash déterministe de
-- la forme désirée (kind + signature + post-conditions triées) calculé par
-- ContractNode::shape_hash — la racine du Merkle. `observed_shape_hash` est la
-- forme IST-OBSERVÉE capturée à la dernière baseline (S6) : la réconciliation
-- compare le ré-calcul live contre cette baseline pour typer le drift.
CREATE TABLE IF NOT EXISTS soll.Contract (
    id                  TEXT PRIMARY KEY,
    project_code        TEXT NOT NULL DEFAULT '',
    kind                TEXT NOT NULL,                 -- module | interface | function | type
    signature           TEXT NOT NULL,                 -- promesse typée (doit contenir '->')
    why                 TEXT NOT NULL DEFAULT '',      -- 'SOLVES <SOLL-id>' — le pourquoi gouvernant
    post_conditions     JSONB NOT NULL DEFAULT '[]'::jsonb,  -- tableau de prédicats (post-conditions promises)
    proves_ref          TEXT NOT NULL DEFAULT '',      -- identité du bundle `proves`
    realized_by         TEXT,                          -- ist.Symbol.id (ancre d'identité ; NULL = planned)
    shape_hash          TEXT NOT NULL,                 -- hash déterministe de la forme DÉSIRÉE (racine Merkle)
    observed_shape_hash TEXT,                          -- baseline IST-OBSERVÉE (S6 ; NULL = jamais réconcilié)
    -- canal sceau (séparé du canal empirique : l'attestation reste HORS-hash) ──
    adequacy_verdict    TEXT,                          -- 'adequate' | 'inadequate' | NULL (inconnu)
    seal_hash           TEXT,                          -- sceau structurel Merkle (NULL = non scellé)
    seal_revision       BIGINT,                        -- version monotone du sceau (millis) — NULL si non scellé
    status              TEXT NOT NULL DEFAULT 'planned', -- planned | bound | sealed | drifted
    created_at          BIGINT NOT NULL DEFAULT (extract(epoch from now()) * 1000)::BIGINT,
    updated_at          BIGINT NOT NULL DEFAULT (extract(epoch from now()) * 1000)::BIGINT
);

-- Réconciliation S6 : index sur l'ancre d'identité (lookup contrat↔symbole).
CREATE INDEX IF NOT EXISTS contract_realized_by_idx ON soll.Contract (realized_by);

-- ── Les arêtes typées du graphe de contrats ───────────────────────────
-- Composite PK (même idiome que soll.Edge) : un même couple peut porter plusieurs
-- relations. relation_type ∈ {CHILD, SOLVES, EXPLAINS, REALIZED_BY}.
--   CHILD       : source = parent-contrat, target = enfant-contrat (agrégat Merkle).
--   SOLVES/     : source = contrat, target = soll.Node (REQ/DEC/… gouvernant).
--   EXPLAINS
--   REALIZED_BY : source = contrat, target = ist.Symbol (cross-ref binding S4).
CREATE TABLE IF NOT EXISTS soll.ContractEdge (
    source_id     TEXT NOT NULL,
    target_id     TEXT NOT NULL,
    relation_type TEXT NOT NULL,
    project_code  TEXT NOT NULL DEFAULT '',
    metadata      JSONB,
    PRIMARY KEY (source_id, target_id, relation_type)
);

CREATE INDEX IF NOT EXISTS contractedge_source_idx ON soll.ContractEdge (source_id);
CREATE INDEX IF NOT EXISTS contractedge_target_idx ON soll.ContractEdge (target_id);
