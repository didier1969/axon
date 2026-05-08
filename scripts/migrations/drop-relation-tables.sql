-- MIL-AXO-015 B.4 — DROP SQL relation tables (FINAL, IRREVERSIBLE step)
--
-- This migration drops the CALLS, CALLS_NIF, and CONTAINS SQL relation
-- tables. After this point, AGE Cypher is the sole reader/writer for
-- graph edges. The B.3 readers (REQ-AXO-215) all have SQL fallbacks
-- which become permanently unreachable post-migration; remove the
-- fallback paths in a follow-up commit (REQ-AXO-216 part 2) once the
-- migration is rolled out and validated.
--
-- Pre-requisites — DO NOT RUN until ALL the following are met:
--
-- 1. AXON_AGE_DUAL_WRITE has been ON in production for >= 1 month
--    so the AGE graph has every relation seen during normal ingestion.
-- 2. AXON_AGE_READ has been ON (default since c88a5fe) and validated:
--    every reader has been observed returning correct results from AGE
--    on production traffic over multiple deploys / sessions.
-- 3. AXON_AGE_ONLY_RELATIONS has been ON for >= 1 week and indexer
--    runs have shown no panics / data-loss alerts.
-- 4. Backup of CALLS / CALLS_NIF / CONTAINS tables taken (see scripts
--    in this same dir for export-to-parquet helper).
-- 5. Operator has confirmed parity counts:
--      AGE.CALLS == SQL.CALLS, etc.
--
-- ⚠️ THIS IS DESTRUCTIVE. There is no automatic rollback once the SQL
-- tables are dropped. Re-creating them from AGE is possible but
-- requires running the dual-write writer in reverse against the AGE
-- graph snapshots, which is not implemented as a one-click operation.
--
-- To run:
--   psql $AXON_LIVE_DATABASE_URL -f scripts/migrations/drop-relation-tables.sql
--
-- REQ-AXO-216 (B.4 final step) — operator action.

\echo '=== Pre-flight: SQL vs AGE relation counts ==='

\echo '-- SQL counts'
SELECT 'CALLS_sql' AS source, count(*) FROM public.CALLS
UNION ALL
SELECT 'CALLS_NIF_sql', count(*) FROM public.CALLS_NIF
UNION ALL
SELECT 'CONTAINS_sql', count(*) FROM public.CONTAINS;

\echo '-- AGE counts (compare manually before proceeding)'
LOAD 'age';
SET search_path = ag_catalog, "$user", public;
SELECT 'CALLS_age' AS source, * FROM cypher('axon_graph',
    $$ MATCH ()-[r:CALLS]->() RETURN count(r) AS n $$) AS (n agtype);
SELECT 'CALLS_NIF_age', * FROM cypher('axon_graph',
    $$ MATCH ()-[r:CALLS_NIF]->() RETURN count(r) AS n $$) AS (n agtype);
SELECT 'CONTAINS_age', * FROM cypher('axon_graph',
    $$ MATCH ()-[r:CONTAINS]->() RETURN count(r) AS n $$) AS (n agtype);

\echo ''
\echo '⚠️  STOP HERE if SQL and AGE counts differ.'
\echo ''
\echo 'To proceed with the irreversible DROP, uncomment the BEGIN/COMMIT'
\echo 'block below and re-run this script. The block is left commented so'
\echo 'an accidental run on production cannot destroy data without a'
\echo 'deliberate edit.'
\echo ''

-- BEGIN;
--
-- DROP TABLE IF EXISTS public.CALLS_NIF CASCADE;
-- DROP TABLE IF EXISTS public.CALLS CASCADE;
-- DROP TABLE IF EXISTS public.CONTAINS CASCADE;
--
-- -- Confirm drops
-- \dt public.CALLS*
-- \dt public.CONTAINS
--
-- COMMIT;

\echo 'No drops applied. Edit the script to uncomment the BEGIN/COMMIT'
\echo 'block when ready.'
