-- REQ-AXO-901624 — P4 Lazy Async TSV Build via pgmq.
--
-- Mesure P1 EXPLAIN ANALYZE session 48 : `content_tsv` GENERATED ALWAYS
-- STORED column = 95 % du coût Chunk INSERT (198 ms → 9 ms si la
-- colonne est retirée, 500-row batch). Cette migration sort le calcul
-- du chemin critique A3 en le déférant à un worker out-of-band qui
-- draine `pgmq.tsv_pending`.
--
-- Self-gating : si pgmq n'est pas dans `pg_available_extensions`
-- (devenv pas encore rebuild avec `exts.pgmq`), la migration devient
-- no-op et le pipeline garde la colonne GENERATED active. Une fois
-- `devenv up -d` exécuté avec la nouvelle `devenv.nix`, ce DDL applique
-- la migration au boot suivant du brain (idempotent — replay safe).
--
-- Idempotence : `IF NOT EXISTS` partout, `CREATE OR REPLACE` pour
-- fonctions/triggers, `DROP EXPRESSION IF EXISTS` pour la colonne.
--
-- ROLLBACK procedure :
--   PG 17 ne supporte pas `ALTER COLUMN ... SET EXPRESSION AS (...)
--   STORED` directement sur une colonne stockée existante. Le rollback
--   nécessite donc :
--     1. DROP TRIGGER trg_chunk_enqueue_tsv ON public.Chunk;
--     2. DROP FUNCTION public.fn_chunk_enqueue_tsv();
--     3. ALTER TABLE public.Chunk DROP COLUMN content_tsv;
--     4. ALTER TABLE public.Chunk ADD COLUMN content_tsv tsvector
--        GENERATED ALWAYS AS (
--            setweight(to_tsvector('simple', coalesce(chunk_path, '')), 'A')
--         || setweight(to_tsvector('simple', coalesce(kind, '')), 'A')
--         || setweight(to_tsvector('english', coalesce(content, '')), 'B')
--         || setweight(to_tsvector('simple', coalesce(file_path, '')), 'C')
--        ) STORED;
--     5. CREATE INDEX idx_chunk_content_tsv ON public.Chunk USING GIN(content_tsv);
--   Coût : réécriture full-table (O(N)). Faire en off-hours.
--   La queue pgmq.tsv_pending et la fn axon.compute_chunk_tsv peuvent
--   rester en place (inertes après step 1) ou être droppées séparément.

DO $migration$
DECLARE
    has_pgmq boolean;
BEGIN
    SELECT EXISTS(
        SELECT 1 FROM pg_available_extensions WHERE name = 'pgmq'
    ) INTO has_pgmq;

    IF NOT has_pgmq THEN
        RAISE WARNING 'REQ-AXO-901624: pgmq extension unavailable in this PG installation. P4 Lazy Async TSV Build migration skipped. Add `exts.pgmq` to devenv.nix services.postgres.extensions and run `devenv up -d`.';
        RETURN;
    END IF;

    -- 1. Install pgmq extension (idempotent).
    EXECUTE 'CREATE EXTENSION IF NOT EXISTS pgmq';

    -- 2. Create the tsv_pending queue (idempotent). pgmq stockes
    --    queues comme tables `pgmq.q_<queue_name>` ; on check directement
    --    information_schema plutôt que `pgmq.list_queues()` dont la
    --    signature a bougé entre versions pgmq.
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.tables
        WHERE table_schema = 'pgmq' AND table_name = 'q_tsv_pending'
    ) THEN
        PERFORM pgmq.create('tsv_pending');
    END IF;

    -- 3. axon schema for helper SQL functions.
    EXECUTE 'CREATE SCHEMA IF NOT EXISTS axon';

    -- 4. axon.compute_chunk_tsv — canonical 4-setweight tsvector
    --    expression, previously inlined in the GENERATED ALWAYS column.
    --    Both the TsvBuilderWorker UPDATE path and any manual backfill
    --    call this function so ts_rank_cd weighting (chunk_path A,
    --    kind A, content B, file_path C) stays centralized.
    EXECUTE $exec$
        CREATE OR REPLACE FUNCTION axon.compute_chunk_tsv(
            p_chunk_path text,
            p_kind       text,
            p_content    text,
            p_file_path  text
        ) RETURNS tsvector
        LANGUAGE sql
        IMMUTABLE
        PARALLEL SAFE
        AS $body$
            SELECT setweight(to_tsvector('simple', coalesce(p_chunk_path, '')), 'A')
                || setweight(to_tsvector('simple', coalesce(p_kind, '')), 'A')
                || setweight(to_tsvector('english', coalesce(p_content, '')), 'B')
                || setweight(to_tsvector('simple', coalesce(p_file_path, '')), 'C');
        $body$;
    $exec$;

    -- 5. Drop the GENERATED expression on content_tsv so the worker can
    --    write into it. Existing rows keep their tsvector values
    --    verbatim (PG ALTER TABLE ... DROP EXPRESSION semantics).
    --    Post-migration, new INSERTs land with content_tsv = NULL until
    --    the worker fills them.
    EXECUTE 'ALTER TABLE public.Chunk ALTER COLUMN content_tsv DROP EXPRESSION IF EXISTS';

    -- 6. Trigger function : fires AFTER INSERT or AFTER UPDATE OF
    --    content. Targeting OF content (not all UPDATE) so the worker's
    --    own UPDATE-of-content_tsv path doesn't recurse.
    EXECUTE $exec$
        CREATE OR REPLACE FUNCTION public.fn_chunk_enqueue_tsv()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $body$
        BEGIN
            PERFORM pgmq.send(
                'tsv_pending',
                jsonb_build_object('chunk_id', NEW.id)
            );
            RETURN NULL;
        END;
        $body$;
    $exec$;

    -- 7. REQ-AXO-91562 — atomic CREATE OR REPLACE TRIGGER (PG 14+).
    --    REQ-AXO-901624 review fix : le WHEN guard évite la cascade de
    --    fake-dirty enqueues qui auraient lieu sinon. A3 UPSERT fait
    --    `ON CONFLICT DO UPDATE SET content = EXCLUDED.content` ce qui
    --    set content unconditionnellement → `UPDATE OF content` se
    --    déclencherait à chaque ré-index même si content n'a pas
    --    bougé. Le content_hash (déjà filé par A3) est l'invariant qui
    --    distingue real-change vs no-op. Sur INSERT (OLD=NULL) on fire
    --    toujours.
    EXECUTE $exec$
        CREATE OR REPLACE TRIGGER trg_chunk_enqueue_tsv
            AFTER INSERT OR UPDATE OF content ON public.Chunk
            FOR EACH ROW
            WHEN (TG_OP = 'INSERT' OR NEW.content_hash IS DISTINCT FROM OLD.content_hash)
            EXECUTE FUNCTION public.fn_chunk_enqueue_tsv()
    $exec$;
END;
$migration$;
