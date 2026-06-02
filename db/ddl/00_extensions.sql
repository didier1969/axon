-- Axon canonical schema — PostgreSQL extensions.
-- Loaded first: every downstream file relies on `vector(N)` types and the
-- pg_trgm opclasses. Idempotent: safe to re-run on every startup.
--
-- REQ-AXO-901863 — extensions are pinned to `public` (an always-present,
-- never-role-named schema) instead of landing wherever the first search_path
-- entry happened to point at CREATE time. Root cause of the old coupling: a
-- bare `CREATE EXTENSION IF NOT EXISTS vector;` with no SCHEMA clause created
-- the type in the role-named schema (`axon` = "$user"), forcing every
-- database to carry `"$user"` on its search_path just to resolve `vector`.
-- Both extensions are relocatable (pg_extension.extrelocatable = t, verified
-- 2026-06-03), so existing installs are migrated in place with
-- ALTER EXTENSION … SET SCHEMA. This eliminates the entire class: a fresh
-- install lands in `public` deterministically and the path no longer needs
-- the role schema.

-- vector (pgvector): mandatory — every IST embedding column is vector(1024).
DO $do$
DECLARE cur text;
BEGIN
    SELECT n.nspname INTO cur
      FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace
     WHERE e.extname = 'vector';
    IF cur IS NULL THEN
        CREATE EXTENSION vector SCHEMA public;
    ELSIF cur <> 'public' THEN
        ALTER EXTENSION vector SET SCHEMA public;
    END IF;
END
$do$;

-- pg_trgm powers GIN trigram indexes on soll.Node.title / description
-- (used by soll_query_context fuzzy lookups). Optional: on minimal PG
-- installs without contrib privileges the bootstrap continues and SOLL
-- fuzzy search is disabled while exact B-tree lookups keep working.
DO $do$
DECLARE cur text;
BEGIN
    SELECT n.nspname INTO cur
      FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace
     WHERE e.extname = 'pg_trgm';
    IF cur IS NULL THEN
        CREATE EXTENSION pg_trgm SCHEMA public;
    ELSIF cur <> 'public' THEN
        ALTER EXTENSION pg_trgm SET SCHEMA public;
    END IF;
EXCEPTION
    WHEN insufficient_privilege THEN
        RAISE NOTICE 'pg_trgm unavailable (insufficient_privilege); soll fuzzy search disabled.';
    WHEN feature_not_supported THEN
        RAISE NOTICE 'pg_trgm unavailable (feature_not_supported); soll fuzzy search disabled.';
    WHEN OTHERS THEN
        RAISE NOTICE 'pg_trgm unavailable (%); soll fuzzy search disabled.', SQLERRM;
END
$do$;

-- REQ-AXO-901860 / REQ-AXO-901863: put `ist` first on the search_path,
-- before 01, so all downstream DDL + the runtime resolve IST tables
-- unqualified; `public` second resolves the vector/pg_trgm extensions now
-- relocated there. The role schema ("$user") is no longer on the path: with
-- the extensions in `public` nothing canonical lives in it (only stray manual
-- scratch tables, which are debt, never load-bearing).
--
-- ALTER DATABASE (not ALTER ROLE): the `axon` role is shared by the dev and
-- live instances, so a role-level ALTER on one silently rewrites the other's
-- search_path and can crash it (incident 2026-06-03: live brain killed after
-- a role-level set dropped "$user"). ALTER DATABASE is persistent across pool
-- resets AND instance-isolated.
DO $do$
BEGIN
    EXECUTE format(
        'ALTER DATABASE %I SET search_path = ist, public',
        current_database()
    );
END
$do$;
