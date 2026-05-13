-- Axon canonical schema — extensions + AGE graph bootstrap (DEC-AXO-082).
-- Idempotent: safe to re-run on every startup.
--
-- Loaded first because every later file relies on `vector(N)` types or
-- `LOAD 'age'` + the canonical `axon_graph` graph.

CREATE EXTENSION IF NOT EXISTS age;
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

-- The single global AGE graph hosting structural edges.
-- (CONTAINS / CALLS / CALLS_NIF / IMPACTS / SUBSTANTIATES — phase B.2
-- writer migration target; vertices for File / Symbol / Chunk are
-- mirrored from the SQL tables which remain authoritative for indexed
-- attribute lookups + pgvector ANN.)
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM ag_catalog.ag_graph WHERE name = 'axon_graph') THEN
        PERFORM ag_catalog.create_graph('axon_graph');
    END IF;
END
$$;

-- AGE labels — idempotent because `create_vlabel` / `create_elabel`
-- raise on duplicate. We catch any "already exists" error so re-runs
-- on a populated graph are safe.
DO $$
DECLARE
    lbl TEXT;
    kind TEXT;
BEGIN
    FOR kind, lbl IN VALUES
        ('vlabel', 'File'),
        ('vlabel', 'Symbol'),
        ('vlabel', 'Chunk'),
        ('elabel', 'CONTAINS'),
        ('elabel', 'CALLS'),
        ('elabel', 'CALLS_NIF'),
        ('elabel', 'IMPACTS'),
        ('elabel', 'SUBSTANTIATES')
    LOOP
        BEGIN
            IF kind = 'vlabel' THEN
                PERFORM ag_catalog.create_vlabel('axon_graph', lbl);
            ELSE
                PERFORM ag_catalog.create_elabel('axon_graph', lbl);
            END IF;
        EXCEPTION
            WHEN duplicate_table THEN NULL;
            WHEN duplicate_object THEN NULL;
            WHEN sqlstate '42P07' THEN NULL;
            WHEN OTHERS THEN
                IF SQLERRM LIKE '%already exists%' THEN
                    NULL;
                ELSE
                    RAISE;
                END IF;
        END;
    END LOOP;
END
$$;
