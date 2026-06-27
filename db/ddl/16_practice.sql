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
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at   TIMESTAMPTZ NOT NULL DEFAULT now(),  -- FSRS review anchor
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- REQ-AXO-902136 — idempotent migration for ALREADY-EXISTING instances (the
-- CREATE TABLE above is a no-op once the table exists; this back-fills `dense`).
ALTER TABLE axon.practice ADD COLUMN IF NOT EXISTS dense TEXT NOT NULL DEFAULT '';
-- REQ-AXO-902138 — consolidation tier (episode → rule → principle).
ALTER TABLE axon.practice ADD COLUMN IF NOT EXISTS tier TEXT NOT NULL DEFAULT 'episode';

-- PR-1 dedup: same scope + same practice text = idempotent (UPSERT reinforces, no dup).
CREATE UNIQUE INDEX IF NOT EXISTS practice_scope_practice_idx
    ON axon.practice (scope, md5(practice));

-- recall: scoped ANN (same HNSW params as ist.ChunkEmbedding) + a partial index so a
-- scoped exact-scan over active rows is cheap (exact scan bypasses HNSW corruption,
-- the REQ-AXO-902129 lesson).
CREATE INDEX IF NOT EXISTS practice_embedding_hnsw_idx
    ON axon.practice USING hnsw (embedding vector_cosine_ops) WITH (m = 16, ef_construction = 64);
CREATE INDEX IF NOT EXISTS practice_scope_active_idx
    ON axon.practice (scope, status) WHERE status = 'active';
CREATE INDEX IF NOT EXISTS practice_tick_idx
    ON axon.practice (status, last_used_at) WHERE status = 'active';
