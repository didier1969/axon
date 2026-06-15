SET search_path = ist, public, "$user";

-- Axon canonical schema — dashboard state cache (REQ-AXO-901806).
-- Idempotent: safe to re-run on every startup.
--
-- Purpose
-- =======
-- The dashboard renders ~52 fields populated by per-project aggregates
-- and runtime telemetry. Per-project aggregates are expensive PG
-- queries (~100 ms with current indices). To serve dashboard_state_v1
-- events at 1 Hz from the brain without hammering PG, the heavy
-- aggregate work is encapsulated in plpgsql functions backed by a
-- canonical `dashboard_cache` table with TTL.
--
-- Design choices
-- ==============
-- * Cache lives in PG, not in brain RAM. Survives brain restart.
--   Multi-instance brain ready (shared cache via PG).
-- * Default TTL 5 s — per-project counts change slowly relative to a
--   1 Hz dashboard refresh ; 5 s lag is imperceptible to humans.
-- * Functions use plpgsql for conditional cache hit/miss logic ; the
--   aggregate itself is pure SQL inside the function body.
-- * jsonb return type so brain just `SELECT … → Json` without column
--   reshaping.

-- ── Cache table ──────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS axon.dashboard_cache (
    cache_key   TEXT PRIMARY KEY,
    data        JSONB NOT NULL,
    computed_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);

COMMENT ON TABLE axon.dashboard_cache IS
    'REQ-AXO-901806 — TTL-cached dashboard aggregates. Single-row-per-key cache backing dashboard_state_v1 event composition.';

-- ── per-project counts function ──────────────────────────────────────

CREATE OR REPLACE FUNCTION axon.dashboard_per_project_counts(ttl_secs INT DEFAULT 5)
RETURNS JSONB
LANGUAGE plpgsql
AS $func$
DECLARE
    cached     JSONB;
    age_secs   NUMERIC;
BEGIN
    SELECT data, EXTRACT(EPOCH FROM (clock_timestamp() - computed_at))
    INTO cached, age_secs
    FROM axon.dashboard_cache
    WHERE cache_key = 'per_project_counts';

    IF cached IS NOT NULL AND age_secs < ttl_secs THEN
        RETURN cached;
    END IF;

    -- ── recompute: project the canonical axon.project_telemetry view ──
    -- Single source of truth (REQ-AXO-901865): totals + per_project + MCP
    -- tools all project THIS view, so they can never diverge. Per-project
    -- file counts ARE available (IndexedFile.project_code exists — the old
    -- "not available from PG" note was stale residue). `files_chunked` =
    -- real A-pipeline coverage (files with >=1 chunk), NOT the retired
    -- status column (REQ-AXO-289).
    SELECT COALESCE(
        jsonb_agg(jsonb_build_object(
            'project_code',  project_code,
            'files_total',   files_total,
            'files_chunked', files_chunked,
            'files_indexed', files_indexed,
            'symbols',       symbols,
            'edges',         edges,
            'chunks',        chunks_total,
            'embedded',      chunks_embedded,
            'fts',           chunks_fts,
            'coverage_pct',  CASE WHEN chunks_total > 0 THEN LEAST(100.0::numeric, ROUND(chunks_embedded::numeric * 100.0 / chunks_total::numeric, 2)) ELSE 0::numeric END
        ) ORDER BY chunks_total DESC),
        '[]'::jsonb
    )
    INTO cached
    FROM axon.project_telemetry;

    INSERT INTO axon.dashboard_cache(cache_key, data, computed_at)
    VALUES ('per_project_counts', cached, clock_timestamp())
    ON CONFLICT (cache_key) DO UPDATE
        SET data = EXCLUDED.data, computed_at = EXCLUDED.computed_at;

    RETURN cached;
END;
$func$;

COMMENT ON FUNCTION axon.dashboard_per_project_counts(INT) IS
    'REQ-AXO-901806 — Per-project aggregates with TTL cache. Returns jsonb array.';

-- ── totals function (sums across all projects) ───────────────────────

CREATE OR REPLACE FUNCTION axon.dashboard_totals(ttl_secs INT DEFAULT 5)
RETURNS JSONB
LANGUAGE plpgsql
AS $func$
DECLARE
    cached     JSONB;
    age_secs   NUMERIC;
    pp         JSONB;
BEGIN
    SELECT data, EXTRACT(EPOCH FROM (clock_timestamp() - computed_at))
    INTO cached, age_secs
    FROM axon.dashboard_cache
    WHERE cache_key = 'totals';

    IF cached IS NOT NULL AND age_secs < ttl_secs THEN
        RETURN cached;
    END IF;

    -- Re-use per_project cache (cheap if warm, expensive if cold) to
    -- compute totals — single source of truth ; rounding consistent.
    pp := axon.dashboard_per_project_counts(ttl_secs);

    -- Canonical funnel (REQ-AXO-901865) — every sum projects the SAME
    -- axon.project_telemetry rows (via pp), so totals and per_project can
    -- never diverge, and the funnel is monotone by construction:
    --   files         = enrolled in IndexedFile (per-project sum)
    --   files_chunked = enrolled files with >=1 chunk (real A-pipeline
    --                   coverage ; the retired status column is gone).
    SELECT jsonb_build_object(
        'projects',        jsonb_array_length(pp),
        'files',           COALESCE((SELECT SUM((p->>'files_total')::bigint)   FROM jsonb_array_elements(pp) p), 0),
        'files_chunked',   COALESCE((SELECT SUM((p->>'files_chunked')::bigint) FROM jsonb_array_elements(pp) p), 0),
        -- REQ-AXO-901890 — processed-files subtotal (status='indexed') for the
        -- 5-box funnel: Indexed = Chunked + No symbols ; Remaining = files - Indexed.
        'files_indexed',   COALESCE((SELECT SUM((p->>'files_indexed')::bigint) FROM jsonb_array_elements(pp) p), 0),
        'symbols',         COALESCE((SELECT SUM((p->>'symbols')::bigint)  FROM jsonb_array_elements(pp) p), 0),
        'edges',           COALESCE((SELECT SUM((p->>'edges')::bigint)    FROM jsonb_array_elements(pp) p), 0),
        'chunks',          COALESCE((SELECT SUM((p->>'chunks')::bigint)   FROM jsonb_array_elements(pp) p), 0),
        'embedded',        COALESCE((SELECT SUM((p->>'embedded')::bigint) FROM jsonb_array_elements(pp) p), 0),
        'fts',             COALESCE((SELECT SUM((p->>'fts')::bigint)      FROM jsonb_array_elements(pp) p), 0),
        -- Canonical throughput proof (REQ-AXO-901865 family) : embeddings
        -- written to ist.ChunkEmbedding in the last 60 s. PG-derived from the
        -- actual table, process-independent, survives indexer restarts — the
        -- dashboard rate is sourced from THIS, never a brain-local snapshot.
        'embedded_60s', (
            SELECT COUNT(*)
            FROM ist.chunkembedding
            WHERE embedded_at_ms > (extract(epoch FROM now()) * 1000)::bigint - 60000
        )::bigint,
        -- REQ-AXO-901807 G2 — visibility on schema drift (orphan
        -- embeddings whose source chunk is gone). When > 0, dashboard
        -- shows a warning ; operator decides when to clean. Cheap
        -- query : indexed antijoin via chunkembedding_pkey + chunk.id.
        'orphan_embeddings', (
            SELECT COUNT(*)
            FROM ist.chunkembedding ce
            WHERE NOT EXISTS (SELECT 1 FROM ist.chunk c WHERE c.id = ce.chunk_id)
        )::bigint
    )
    INTO cached;

    -- coverage_pct = embedded / chunks * 100 (avoid div/0). Clamped to
    -- 100.0 so transient `embedded > chunks` regimes (orphan embeddings,
    -- chunks deleted but embeddings retained pre-cleanup) cannot surface
    -- a > 100 % ratio in the dashboard. Inspect `orphan_embeddings` to
    -- detect the drift directly.
    cached := cached || jsonb_build_object(
        'coverage_pct',
        CASE
            WHEN (cached->>'chunks')::bigint > 0
            THEN LEAST(100.0::numeric, ROUND((cached->>'embedded')::numeric * 100.0 / (cached->>'chunks')::numeric, 2))
            ELSE 0
        END,
        'pending',
        GREATEST(0, (cached->>'chunks')::bigint - (cached->>'embedded')::bigint),
        -- Canonical embedding rate (chunks/s) = embedded_60s / 60. Single
        -- source of truth for the dashboard "chunks/sec" panel ; honest 0.0
        -- only when nothing was embedded in the last minute.
        'embeddings_per_second',
        ROUND((cached->>'embedded_60s')::numeric / 60.0, 2)
    );

    INSERT INTO axon.dashboard_cache(cache_key, data, computed_at)
    VALUES ('totals', cached, clock_timestamp())
    ON CONFLICT (cache_key) DO UPDATE
        SET data = EXCLUDED.data, computed_at = EXCLUDED.computed_at;

    RETURN cached;
END;
$func$;

COMMENT ON FUNCTION axon.dashboard_totals(INT) IS
    'REQ-AXO-901806 — Aggregate totals across all projects with TTL cache. Returns jsonb object including coverage_pct and pending.';

-- ── Runtime config snapshot (boot-written by indexer) ────────────────
-- REQ-AXO-901806 F2 — semi-static configs (worker counts, batch sizes,
-- notify_channel, a3_to_b1_buffer_cap) live in PG so
-- dashboard_state_full() returns the full picture in a single call.
-- Indexer writes UPSERT once at boot ; brain reads via the composite
-- function below.
--
-- Why PG: aligns with PIL-AXO-009 (PG canonical) and avoids passing
-- 15+ config args from main_telemetry.rs → compose_dashboard_state_v1
-- on every 1 Hz tick.

CREATE TABLE IF NOT EXISTS axon.runtime_config_snapshot (
    runtime_role  TEXT PRIMARY KEY,
    config        JSONB NOT NULL,
    written_at    TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);

COMMENT ON TABLE axon.runtime_config_snapshot IS
    'REQ-AXO-901806 F2 — boot-time semi-static runtime configs (pipeline_a/b workers + batch sizes, notify_channel, coldstart_poll_interval_secs, a3_to_b1_buffer_cap, ingress_drain_batch). UPSERT by runtime_role at process startup.';

-- ── Composite dashboard_state_full function ──────────────────────────
-- REQ-AXO-901806 — single PG round-trip for the dashboard. Composes
-- the existing TTL-cached aggregates with the runtime config snapshot
-- so the brain's `compose_dashboard_state_v1` only needs to add live
-- in-memory metrics (rates, queues, scheduler state, embedder state,
-- identity) to assemble the full event.

CREATE OR REPLACE FUNCTION axon.dashboard_state_full(p_ttl_secs INT DEFAULT 5)
RETURNS JSONB
LANGUAGE plpgsql
AS $func$
DECLARE
    cfg JSONB;
BEGIN
    SELECT config
    INTO cfg
    FROM axon.runtime_config_snapshot
    WHERE runtime_role = 'indexer'
    LIMIT 1;

    RETURN jsonb_build_object(
        'totals',         axon.dashboard_totals(p_ttl_secs),
        'per_project',    axon.dashboard_per_project_counts(p_ttl_secs),
        'runtime_config', COALESCE(cfg, '{}'::jsonb)
    );
END;
$func$;

COMMENT ON FUNCTION axon.dashboard_state_full(INT) IS
    'REQ-AXO-901806 — Composite dashboard state from TTL-cached aggregates + runtime config snapshot. Single round-trip for the brain 1 Hz tick.';
