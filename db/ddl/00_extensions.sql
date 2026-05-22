-- Axon canonical schema — PostgreSQL extensions.
-- Loaded first: every downstream file relies on `vector(N)` types.
-- Idempotent: safe to re-run on every startup.

CREATE EXTENSION IF NOT EXISTS vector;

-- pg_trgm powers GIN trigram indexes on soll.Node.title / description
-- (used by soll_query_context fuzzy lookups). Optional: on minimal PG
-- installs without contrib privileges the bootstrap continues and SOLL
-- fuzzy search is disabled while exact B-tree lookups keep working.
DO $$
BEGIN
    CREATE EXTENSION IF NOT EXISTS pg_trgm;
EXCEPTION
    WHEN insufficient_privilege THEN
        RAISE NOTICE 'pg_trgm unavailable (insufficient_privilege); soll fuzzy search disabled.';
    WHEN feature_not_supported THEN
        RAISE NOTICE 'pg_trgm unavailable (feature_not_supported); soll fuzzy search disabled.';
    WHEN OTHERS THEN
        RAISE NOTICE 'pg_trgm unavailable (%); soll fuzzy search disabled.', SQLERRM;
END
$$;
