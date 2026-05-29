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

CREATE TABLE IF NOT EXISTS axon_runtime.dashboard_cache (
    cache_key   TEXT PRIMARY KEY,
    data        JSONB NOT NULL,
    computed_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);

COMMENT ON TABLE axon_runtime.dashboard_cache IS
    'REQ-AXO-901806 — TTL-cached dashboard aggregates. Single-row-per-key cache backing dashboard_state_v1 event composition.';

-- ── per-project counts function ──────────────────────────────────────

CREATE OR REPLACE FUNCTION axon_runtime.dashboard_per_project_counts(ttl_secs INT DEFAULT 5)
RETURNS JSONB
LANGUAGE plpgsql
AS $func$
DECLARE
    cached     JSONB;
    age_secs   NUMERIC;
BEGIN
    SELECT data, EXTRACT(EPOCH FROM (clock_timestamp() - computed_at))
    INTO cached, age_secs
    FROM axon_runtime.dashboard_cache
    WHERE cache_key = 'per_project_counts';

    IF cached IS NOT NULL AND age_secs < ttl_secs THEN
        RETURN cached;
    END IF;

    -- ── recompute ────────────────────────────────────────────────────
    -- Schema note: chunk, chunkembedding, symbol, edge all carry
    -- project_code. IndexedFile does NOT (REQ-AXO-289 streaming v2 keys
    -- it by path only ; project is derived from path elsewhere). Per-
    -- project file counts are therefore NOT available from PG — only
    -- chunks/embedded/symbols/edges per project. Total file count is
    -- exposed in `dashboard_totals` from a global indexedfile count.
    WITH per_project AS (
        SELECT
            p.project_code,
            COALESCE(s.symbols,  0) AS symbols,
            COALESCE(e.edges,    0) AS edges,
            COALESCE(c.chunks,   0) AS chunks,
            COALESCE(ce.embedded, 0) AS embedded
        FROM (
            SELECT DISTINCT project_code FROM public.symbol
            UNION SELECT DISTINCT project_code FROM public.chunk
            UNION SELECT DISTINCT project_code FROM public.edge
        ) p
        LEFT JOIN (SELECT project_code, COUNT(*) AS symbols  FROM public.symbol         GROUP BY 1) s  USING (project_code)
        LEFT JOIN (SELECT project_code, COUNT(*) AS edges    FROM public.edge           GROUP BY 1) e  USING (project_code)
        LEFT JOIN (SELECT project_code, COUNT(*) AS chunks   FROM public.chunk          GROUP BY 1) c  USING (project_code)
        LEFT JOIN (SELECT project_code, COUNT(*) AS embedded FROM public.chunkembedding GROUP BY 1) ce USING (project_code)
    )
    SELECT COALESCE(
        jsonb_agg(jsonb_build_object(
            'project_code', project_code,
            'symbols',      symbols,
            'edges',        edges,
            'chunks',       chunks,
            'embedded',     embedded,
            'coverage_pct', CASE WHEN chunks > 0 THEN (embedded::numeric * 100.0 / chunks)::numeric(8,2) ELSE 0::numeric END
        ) ORDER BY chunks DESC),
        '[]'::jsonb
    )
    INTO cached
    FROM per_project;

    INSERT INTO axon_runtime.dashboard_cache(cache_key, data, computed_at)
    VALUES ('per_project_counts', cached, clock_timestamp())
    ON CONFLICT (cache_key) DO UPDATE
        SET data = EXCLUDED.data, computed_at = EXCLUDED.computed_at;

    RETURN cached;
END;
$func$;

COMMENT ON FUNCTION axon_runtime.dashboard_per_project_counts(INT) IS
    'REQ-AXO-901806 — Per-project aggregates with TTL cache. Returns jsonb array.';

-- ── totals function (sums across all projects) ───────────────────────

CREATE OR REPLACE FUNCTION axon_runtime.dashboard_totals(ttl_secs INT DEFAULT 5)
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
    FROM axon_runtime.dashboard_cache
    WHERE cache_key = 'totals';

    IF cached IS NOT NULL AND age_secs < ttl_secs THEN
        RETURN cached;
    END IF;

    -- Re-use per_project cache (cheap if warm, expensive if cold) to
    -- compute totals — single source of truth ; rounding consistent.
    pp := axon_runtime.dashboard_per_project_counts(ttl_secs);

    -- Total file count comes from indexedfile (not per-project — see
    -- note in dashboard_per_project_counts).
    SELECT jsonb_build_object(
        'projects',        jsonb_array_length(pp),
        'files',           (SELECT COUNT(*) FROM public.indexedfile)::bigint,
        'symbols',         COALESCE((SELECT SUM((p->>'symbols')::bigint)  FROM jsonb_array_elements(pp) p), 0),
        'edges',           COALESCE((SELECT SUM((p->>'edges')::bigint)    FROM jsonb_array_elements(pp) p), 0),
        'chunks',          COALESCE((SELECT SUM((p->>'chunks')::bigint)   FROM jsonb_array_elements(pp) p), 0),
        'embedded',        COALESCE((SELECT SUM((p->>'embedded')::bigint) FROM jsonb_array_elements(pp) p), 0),
        -- REQ-AXO-901807 G2 — visibility on schema drift (orphan
        -- embeddings whose source chunk is gone). When > 0, dashboard
        -- shows a warning ; operator decides when to clean. Cheap
        -- query : indexed antijoin via chunkembedding_pkey + chunk.id.
        'orphan_embeddings', (
            SELECT COUNT(*)
            FROM public.chunkembedding ce
            WHERE NOT EXISTS (SELECT 1 FROM public.chunk c WHERE c.id = ce.chunk_id)
        )::bigint
    )
    INTO cached;

    -- coverage_pct = embedded / chunks * 100 (avoid div/0 ; can be > 100
    -- transiently when embeddings outpace chunk source_hash refresh).
    cached := cached || jsonb_build_object(
        'coverage_pct',
        CASE
            WHEN (cached->>'chunks')::bigint > 0
            THEN ROUND((cached->>'embedded')::numeric * 100.0 / (cached->>'chunks')::numeric, 2)
            ELSE 0
        END,
        'pending',
        GREATEST(0, (cached->>'chunks')::bigint - (cached->>'embedded')::bigint)
    );

    INSERT INTO axon_runtime.dashboard_cache(cache_key, data, computed_at)
    VALUES ('totals', cached, clock_timestamp())
    ON CONFLICT (cache_key) DO UPDATE
        SET data = EXCLUDED.data, computed_at = EXCLUDED.computed_at;

    RETURN cached;
END;
$func$;

COMMENT ON FUNCTION axon_runtime.dashboard_totals(INT) IS
    'REQ-AXO-901806 — Aggregate totals across all projects with TTL cache. Returns jsonb object including coverage_pct and pending.';
