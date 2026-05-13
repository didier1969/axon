-- Axon canonical schema — extensions (DEC-AXO-082 / MIL-AXO-017 slice 6B Phase E).
-- Idempotent: safe to re-run on every startup.
--
-- AGE extension retired (DEC-AXO-083) — `public.Edge` is the canonical
-- structural edge storage. Loaded first because every later file relies
-- on `vector(N)` types from pgvector.

CREATE EXTENSION IF NOT EXISTS vector;
-- pg_trgm powers GIN trigram indexes on soll.Node.title / description
-- (used by soll_query_context fuzzy lookups). Optional — wrapped so
-- the bootstrap continues without it on minimal PG installs without
-- contrib (those just lose trigram fuzzy search; exact lookups still
-- work via the B-tree indexes).
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
