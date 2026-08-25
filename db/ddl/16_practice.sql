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

-- REQ-AXO-902131 — CROSS-TENANT BEST-PRACTICE MEMORY (governed, self-improving).
-- Generalises the proven Nexus lesson-loop (DEC-NEX-008) into an Axon product so
-- every project inherits governed best practices. COMPOSES existing Axon surface:
--   WRITE-GATE = contradiction_check (reject a practice contradicting the base),
--   RECALL     = pgvector scoped ANN/exact-scan,
--   SHARE      = mailbox (scope='*' cross-tenant + source_project provenance).
-- NEW (ported from Nexus): Physarum trust + FSRS decay + prune + stagnation monitor.
-- Runtime data (NOT SOLL intent) → `axon` schema, fully reconstructible. Embedding
-- in the same 1024d BGE space as ist.ChunkEmbedding so recall is consistent.
CREATE SCHEMA IF NOT EXISTS axon;

-- PR-1 — the practice store. One row = one governed best practice for a scope.
CREATE TABLE IF NOT EXISTS axon.practice (
    id             BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    scope          TEXT        NOT NULL,             -- project code OR '*' (global/cross-tenant)
    context        TEXT        NOT NULL,             -- the situation signature (embedded for recall)
    practice       TEXT        NOT NULL,             -- the advice/rule itself (prose source)
    dense          TEXT        NOT NULL DEFAULT '',  -- REQ-AXO-902136: caller-provided DENSE form (body_dense-style, pointer-bearing). '' = fall back to `practice`.
    evidence       TEXT        NOT NULL DEFAULT '',  -- pointer-bearing proof (SOLL ids / metric / commit)
    embedding      vector(1024),                     -- context embedding (NULL until embedded)
    -- governance: Physarum trust + FSRS decay state.
    trust          REAL        NOT NULL DEFAULT 0.5, -- Physarum conductivity ∈ [0,1]
    stability      REAL        NOT NULL DEFAULT 1.0, -- FSRS S (days) — grows on reinforcement
    difficulty     REAL        NOT NULL DEFAULT 5.0, -- FSRS D ∈ [1,10]
    use_count      INTEGER     NOT NULL DEFAULT 0,
    win_count      INTEGER     NOT NULL DEFAULT 0,   -- reinforcements with positive usefulness
    source_project TEXT        NOT NULL DEFAULT '',  -- who contributed it (mailbox provenance)
    status         TEXT        NOT NULL DEFAULT 'active', -- active | pruned | merged (never DELETE — audit)
    tier           TEXT        NOT NULL DEFAULT 'episode', -- REQ-AXO-902138: episode → rule → principle (consolidation par maturité)
    perishability  TEXT        NOT NULL DEFAULT 'durable', -- REQ-AXO-902141: durable (best-practice : pas de decay temps) | perishable (FSRS temporel)
    role           TEXT        NOT NULL DEFAULT '*',     -- REQ-AXO-902149: agent/rôle ('*' = partagé entre tous les agents, défaut N1 stigmergie)
    model          TEXT        NOT NULL DEFAULT '*',     -- REQ-AXO-902149: LLM ('*' = agnostique, défaut ; id-modèle = model-specific opt-in H1)
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at   TIMESTAMPTZ NOT NULL DEFAULT now(),  -- FSRS review anchor
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- REQ-AXO-902136 — idempotent migration for ALREADY-EXISTING instances (the
-- CREATE TABLE above is a no-op once the table exists; this back-fills `dense`).
SELECT public.add_column_if_absent('axon', 'practice', 'dense', $def$TEXT NOT NULL DEFAULT ''$def$);
-- REQ-AXO-902138 — consolidation tier (episode → rule → principle).
SELECT public.add_column_if_absent('axon', 'practice', 'tier', $def$TEXT NOT NULL DEFAULT 'episode'$def$);
-- REQ-AXO-902141 — classe de périssabilité (durable = pas de decay temps).
SELECT public.add_column_if_absent('axon', 'practice', 'perishability', $def$TEXT NOT NULL DEFAULT 'durable'$def$);
-- REQ-AXO-902149 — axes de partitionnement multi-agent (rôle + modèle), '*' = partagé/agnostique.
SELECT public.add_column_if_absent('axon', 'practice', 'role',  $def$TEXT NOT NULL DEFAULT '*'$def$);
SELECT public.add_column_if_absent('axon', 'practice', 'model', $def$TEXT NOT NULL DEFAULT '*'$def$);

-- PR-1 dedup: same (scope, role, model) + same practice text = idempotent (UPSERT
-- reinforces, no dup). REQ-AXO-902149 extended the key with role+model so the same
-- prose can coexist across distinct agents/models without UPSERT collision; legacy
-- puts default to ('*','*') so back-compat holds. The old scope-only index is dropped.
DROP INDEX IF EXISTS axon.practice_scope_practice_idx;
CREATE UNIQUE INDEX IF NOT EXISTS practice_scope_role_model_practice_idx
    ON axon.practice (scope, role, model, md5(practice));

-- recall: scoped ANN (same HNSW params as ist.ChunkEmbedding) + a partial index so a
-- scoped exact-scan over active rows is cheap (exact scan bypasses HNSW corruption,
-- the REQ-AXO-902129 lesson).
CREATE INDEX IF NOT EXISTS practice_embedding_hnsw_idx
    ON axon.practice USING hnsw (embedding vector_cosine_ops) WITH (m = 16, ef_construction = 64);
CREATE INDEX IF NOT EXISTS practice_scope_active_idx
    ON axon.practice (scope, status) WHERE status = 'active';
-- REQ-AXO-902149 — partition filter (scope hierarchy ∩ role ∩ model) on active rows.
CREATE INDEX IF NOT EXISTS practice_partition_idx
    ON axon.practice (scope, role, model, status) WHERE status = 'active';
CREATE INDEX IF NOT EXISTS practice_tick_idx
    ON axon.practice (status, last_used_at) WHERE status = 'active';
