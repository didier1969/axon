SET search_path = ist, public, "$user";

-- Axon canonical schema — observable embedder state (DEC-AXO-901626).
--
-- The brain composer derives `embedder_runtime` (status + dashboard_state_v1)
-- by OBSERVATION, never by self-report. This function materialises the
-- PG-canonical half of that observation in a single round-trip:
--   * embedded_60s          — throughput proof; survives indexer restarts
--                             (unlike the in-process AtomicU64 counter that
--                             resets), reflects the real downstream of the
--                             embedder rather than its intent.
--   * embedded_total        — cumulative ChunkEmbedding rows.
--   * oldest_pending_age_s  — staleness signal: age (seconds) of the oldest
--                             file still carrying pending chunks, derived
--                             from IndexedFile.discovered_ms. 0 when no
--                             pending work remains. Feeds the dashboard
--                             freshness verdict.
--
-- The GPU/CPU half of the observation is OS-level (`nvidia-smi` cross-
-- referenced with the indexer pid published in
-- axon.EmbedderLifecycleHeartbeat.pid) and lives in the Rust
-- `observed_gpu` module — it cannot be expressed in SQL.
--
-- Idempotent: CREATE OR REPLACE, safe to re-run on every startup.

CREATE OR REPLACE FUNCTION axon.embedder_observed_state()
RETURNS jsonb
LANGUAGE sql
STABLE
AS $$
    WITH now_ms AS (
        SELECT (extract(epoch FROM now()) * 1000)::bigint AS v
    ),
    embedded_60s AS (
        SELECT count(*)::bigint AS n
        FROM ist.ChunkEmbedding, now_ms
        WHERE embedded_at_ms > now_ms.v - 60000
    ),
    embedded_total AS (
        SELECT count(*)::bigint AS n
        FROM ist.ChunkEmbedding
    ),
    oldest_pending AS (
        SELECT min(f.discovered_ms) AS min_ms
        FROM ist.Chunk c
        JOIN ist.IndexedFile f ON f.path = c.file_path
        WHERE c.embed_status = 'pending'
    )
    SELECT jsonb_build_object(
        'embedded_60s',   (SELECT n FROM embedded_60s),
        'embedded_total', (SELECT n FROM embedded_total),
        'oldest_pending_age_s', COALESCE(
            (SELECT GREATEST(0, (now_ms.v - oldest_pending.min_ms) / 1000)::bigint
               FROM oldest_pending, now_ms
              WHERE oldest_pending.min_ms IS NOT NULL),
            0
        )
    );
$$;
