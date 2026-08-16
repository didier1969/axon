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

-- ═══════════════════════════════════════════════════════════════════════
-- Gardes DDL sans verrou — REQ-AXO-902339
-- ═══════════════════════════════════════════════════════════════════════
-- PostgreSQL prend le verrou de table AVANT d'évaluer le `IF NOT EXISTS`.
-- `ADD COLUMN IF NOT EXISTS`, `CREATE INDEX IF NOT EXISTS`, `DROP TRIGGER
-- IF EXISTS` sont donc idempotents dans leur EFFET, pas dans leur COÛT DE
-- VERROUILLAGE : rejoués sur une base déjà à jour, ils réclament quand même
-- ACCESS EXCLUSIVE (ou SHARE) sur la table.
--
-- Contre l'indexeur live, qui tient un ROW EXCLUSIVE en continu sur
-- ist.Chunk / ist.Symbol / ist.IndexedFile, ce n'est pas une course qu'on
-- peut reperdre puis regagner : c'est une famine. Aucun nombre de tentatives
-- n'ouvre la fenêtre. Le step 5b du promote a échoué deux fois le
-- 2026-08-15 (11:33 et 20:33) sur un schéma pourtant DÉJÀ CORRECT — un
-- promote réussi déclaré FAILED.
--
-- Mesuré, pas supposé : 28 énoncés du DDL canonique bloquent sur le chemin
-- no-op (6 ADD COLUMN, 1 SET DEFAULT, 2 contraintes, 14 index, 5 triggers).
-- Le test de non-régression est `ddl_lock_tests.rs` : il rejoue le DDL entier
-- pendant qu'une session tient les verrous d'écrivain, avec un lock_timeout
-- court, et exige zéro échec. Son contrôle négatif rejoue la forme BRUTE et
-- exige qu'elle échoue — sans quoi la garde ne prouverait rien.
--
-- Les fonctions lisent le catalogue (aucun verrou de table) et n'émettent le
-- DDL que lorsqu'il change réellement quelque chose. Attendre le verrou
-- devient alors légitime : c'est une vraie migration, sur une base qui en a
-- besoin, une seule fois. C'est ce qui rend enfin VRAIE la prémisse du retry
-- de REQ-AXO-902328 (« un lock timeout est une course rejouable ») : une fois
-- la famine-par-no-op retirée, il ne reste que des courses.
--
-- `p_definition` / `p_statement` sont interpolés tels quels : ce sont des
-- littéraux de CE dépôt, jamais une entrée d'appelant. Les identifiants, eux,
-- passent par `%I`.

-- Ajoute une colonne seulement si elle est absente. Retourne true si elle a
-- été ajoutée. Le `IF NOT EXISTS` interne est CONSERVÉ : entre la lecture du
-- catalogue et l'ALTER, un bootstrap concurrent peut avoir gagné.
CREATE OR REPLACE FUNCTION public.add_column_if_absent(
    p_schema     text,
    p_table      text,
    p_column     text,
    p_definition text
) RETURNS boolean
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $fn$
BEGIN
    IF EXISTS (
        SELECT 1
          FROM pg_attribute  a
          JOIN pg_class      c ON c.oid = a.attrelid
          JOIN pg_namespace  n ON n.oid = c.relnamespace
         WHERE n.nspname = lower(p_schema)
           AND c.relname = lower(p_table)
           AND a.attname = lower(p_column)
           AND a.attnum  > 0
           AND NOT a.attisdropped
    ) THEN
        RETURN false;
    END IF;

    EXECUTE format(
        'ALTER TABLE %I.%I ADD COLUMN IF NOT EXISTS %I %s',
        lower(p_schema), lower(p_table), lower(p_column), p_definition
    );
    RETURN true;
END
$fn$;

-- Pose un DEFAULT seulement si la colonne n'en a aucun. Retourne true si posé.
--
-- LIMITE ASSUMÉE : changer l'expression d'un DEFAULT déjà posé ne se propage
-- PAS au prochain boot — il faut un énoncé de migration délibéré. C'est le
-- prix du no-lock, et c'est le bon prix : un `SET DEFAULT` inconditionnel sur
-- une table chaude est précisément le défaut que cette fonction ferme. Le seul
-- appelant (ist.Chunk.created_at_ms, REQ-AXO-902260) veut exactement cette
-- sémantique : ajouter la colonne nue puis poser le défaut pour les insertions
-- FUTURES, une fois.
CREATE OR REPLACE FUNCTION public.set_column_default_if_absent(
    p_schema  text,
    p_table   text,
    p_column  text,
    p_default text
) RETURNS boolean
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $fn$
DECLARE
    v_has_default boolean;
BEGIN
    SELECT a.atthasdef
      INTO v_has_default
      FROM pg_attribute  a
      JOIN pg_class      c ON c.oid = a.attrelid
      JOIN pg_namespace  n ON n.oid = c.relnamespace
     WHERE n.nspname = lower(p_schema)
       AND c.relname = lower(p_table)
       AND a.attname = lower(p_column)
       AND a.attnum  > 0
       AND NOT a.attisdropped;

    -- Colonne absente = faute de programmation dans le DDL, pas un cas nominal :
    -- on échoue fort plutôt que de poser un défaut sur le vide en silence.
    IF v_has_default IS NULL THEN
        RAISE EXCEPTION 'set_column_default_if_absent: %.%.% n''existe pas',
            p_schema, p_table, p_column;
    END IF;

    IF v_has_default THEN
        RETURN false;
    END IF;

    EXECUTE format(
        'ALTER TABLE %I.%I ALTER COLUMN %I SET DEFAULT %s',
        lower(p_schema), lower(p_table), lower(p_column), p_default
    );
    RETURN true;
END
$fn$;

-- Crée un index seulement s'il n'existe pas déjà sous ce nom.
--
-- Sémantiquement IDENTIQUE à `CREATE INDEX IF NOT EXISTS` : PostgreSQL ne
-- compare que le NOM, jamais la définition — modifier une définition en
-- gardant le nom était déjà un no-op silencieux avant ce changement. On retire
-- le verrou, pas une capacité.
CREATE OR REPLACE FUNCTION public.create_index_if_absent(
    p_schema    text,
    p_index     text,
    p_statement text
) RETURNS boolean
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $fn$
BEGIN
    IF EXISTS (
        SELECT 1
          FROM pg_class     c
          JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = lower(p_schema)
           AND c.relname = lower(p_index)
           AND c.relkind IN ('i', 'I')
    ) THEN
        RETURN false;
    END IF;

    EXECUTE p_statement;
    RETURN true;
END
$fn$;

-- Crée un trigger seulement s'il n'existe pas déjà sous ce nom sur cette table.
--
-- LIMITE ASSUMÉE : changer la CLAUSE d'un trigger existant (timing, événements,
-- WHEN) ne se propage pas au prochain boot — renommer le trigger, ou le
-- supprimer explicitement. Le COMPORTEMENT, lui, reste chaud-modifiable : il vit
-- dans `CREATE OR REPLACE FUNCTION`, qui ne verrouille aucune table.
CREATE OR REPLACE FUNCTION public.create_trigger_if_absent(
    p_schema    text,
    p_table     text,
    p_trigger   text,
    p_statement text
) RETURNS boolean
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $fn$
BEGIN
    IF EXISTS (
        SELECT 1
          FROM pg_trigger   t
          JOIN pg_class     c ON c.oid = t.tgrelid
          JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = lower(p_schema)
           AND c.relname = lower(p_table)
           AND t.tgname  = lower(p_trigger)
           AND NOT t.tgisinternal
    ) THEN
        RETURN false;
    END IF;

    EXECUTE p_statement;
    RETURN true;
END
$fn$;
