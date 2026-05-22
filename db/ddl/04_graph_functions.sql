-- Axon canonical schema — graph traversal SQL function library.
--
-- Five SQL functions + one hybrid-retrieval function wrapping
-- WITH RECURSIVE queries on `public.Edge`. Each function is
-- LANGUAGE sql STABLE PARALLEL SAFE so PG can:
--   * cache the plan as a prepared statement,
--   * parallelise execution across workers,
--   * inline simple call-sites.
--
-- All functions are project-aware. Empty `p_project_code` ('') means
-- unscoped (traverse the full graph); a non-empty value scopes to one
-- project's edges.
--
-- Cycle-safe: each WITH RECURSIVE carries a visited-array guard and
-- terminates when depth hits the bound or no new nodes are reachable.
--
-- Idempotent: re-running this file is a no-op (CREATE OR REPLACE).

-- ─────────────────────────────────────────────────────────────────────
-- impact(p_start_id, p_max_depth, p_project_code)
--
-- Forward traversal along outbound edges (source_id → target_id).
-- "What is downstream of start_id, within max_depth hops?"
-- Returns one row per reachable node ordered by (target_id, distance).
-- Powers the MCP `impact` tool ("blast radius of changing this symbol").
-- ─────────────────────────────────────────────────────────────────────
CREATE OR REPLACE FUNCTION public.impact(
    p_start_id     TEXT,
    p_max_depth    INT  DEFAULT 5,
    p_project_code TEXT DEFAULT ''
) RETURNS TABLE (
    target_id     TEXT,
    distance      INT,
    relation_type TEXT
)
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    WITH RECURSIVE walk AS (
        SELECT
            e.target_id,
            1                              AS distance,
            e.relation_type                AS relation_type,
            ARRAY[p_start_id, e.target_id] AS visited
        FROM public.Edge e
        WHERE e.source_id = p_start_id
          AND (p_project_code = '' OR e.project_code = p_project_code)
        UNION
        SELECT
            e.target_id,
            w.distance + 1,
            e.relation_type,
            w.visited || e.target_id
        FROM walk w
        JOIN public.Edge e ON e.source_id = w.target_id
        WHERE w.distance < p_max_depth
          AND NOT (e.target_id = ANY(w.visited))
          AND (p_project_code = '' OR e.project_code = p_project_code)
    )
    SELECT DISTINCT ON (target_id)
        target_id,
        distance,
        relation_type
    FROM walk
    ORDER BY target_id, distance;
$$;

COMMENT ON FUNCTION public.impact(TEXT, INT, TEXT) IS
'Forward traversal on public.Edge: nodes reachable from start_id within max_depth hops. Cycle-safe.';

-- ─────────────────────────────────────────────────────────────────────
-- callers_of(p_target_id, p_max_depth, p_project_code)
--
-- Reverse traversal along inbound edges (target_id ← source_id).
-- "What points TO target_id, within max_depth reverse hops?"
-- Powers the MCP `why` tool's first-level reverse lookup.
-- ─────────────────────────────────────────────────────────────────────
CREATE OR REPLACE FUNCTION public.callers_of(
    p_target_id    TEXT,
    p_max_depth    INT  DEFAULT 1,
    p_project_code TEXT DEFAULT ''
) RETURNS TABLE (
    source_id     TEXT,
    distance      INT,
    relation_type TEXT
)
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    WITH RECURSIVE walk AS (
        SELECT
            e.source_id,
            1                               AS distance,
            e.relation_type                 AS relation_type,
            ARRAY[p_target_id, e.source_id] AS visited
        FROM public.Edge e
        WHERE e.target_id = p_target_id
          AND (p_project_code = '' OR e.project_code = p_project_code)
        UNION
        SELECT
            e.source_id,
            w.distance + 1,
            e.relation_type,
            w.visited || e.source_id
        FROM walk w
        JOIN public.Edge e ON e.target_id = w.source_id
        WHERE w.distance < p_max_depth
          AND NOT (e.source_id = ANY(w.visited))
          AND (p_project_code = '' OR e.project_code = p_project_code)
    )
    SELECT DISTINCT ON (source_id)
        source_id,
        distance,
        relation_type
    FROM walk
    ORDER BY source_id, distance;
$$;

COMMENT ON FUNCTION public.callers_of(TEXT, INT, TEXT) IS
'Reverse traversal on public.Edge: nodes pointing TO target_id within max_depth hops. Cycle-safe.';

-- ─────────────────────────────────────────────────────────────────────
-- why_chain(p_target_id, p_max_depth, p_project_code)
--
-- Same shape as callers_of, but materialises the relation path
-- (concatenated `r1->r2->...`) so the MCP `why` tool can present the
-- chain of reasoning.
-- ─────────────────────────────────────────────────────────────────────
CREATE OR REPLACE FUNCTION public.why_chain(
    p_target_id    TEXT,
    p_max_depth    INT  DEFAULT 5,
    p_project_code TEXT DEFAULT ''
) RETURNS TABLE (
    source_id      TEXT,
    distance       INT,
    relation_chain TEXT
)
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    WITH RECURSIVE walk AS (
        SELECT
            e.source_id,
            1                               AS distance,
            e.relation_type                 AS relation_chain,
            ARRAY[p_target_id, e.source_id] AS visited
        FROM public.Edge e
        WHERE e.target_id = p_target_id
          AND (p_project_code = '' OR e.project_code = p_project_code)
        UNION
        SELECT
            e.source_id,
            w.distance + 1,
            e.relation_type || '->' || w.relation_chain,
            w.visited || e.source_id
        FROM walk w
        JOIN public.Edge e ON e.target_id = w.source_id
        WHERE w.distance < p_max_depth
          AND NOT (e.source_id = ANY(w.visited))
          AND (p_project_code = '' OR e.project_code = p_project_code)
    )
    SELECT DISTINCT ON (source_id)
        source_id,
        distance,
        relation_chain
    FROM walk
    ORDER BY source_id, distance;
$$;

COMMENT ON FUNCTION public.why_chain(TEXT, INT, TEXT) IS
'Reverse traversal with relation-path concatenation. Each row carries the chain of relation_types from source to target.';

-- ─────────────────────────────────────────────────────────────────────
-- blast_radius(p_start_id, p_max_depth, p_project_code)
--
-- Pure counter: COUNT(DISTINCT target_id) reachable from start_id.
-- Useful for ranking impact without paying the cost of returning every
-- node.
-- ─────────────────────────────────────────────────────────────────────
CREATE OR REPLACE FUNCTION public.blast_radius(
    p_start_id     TEXT,
    p_max_depth    INT  DEFAULT 5,
    p_project_code TEXT DEFAULT ''
) RETURNS BIGINT
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT COUNT(DISTINCT target_id)::BIGINT
    FROM public.impact(p_start_id, p_max_depth, p_project_code);
$$;

COMMENT ON FUNCTION public.blast_radius(TEXT, INT, TEXT) IS
'Count of distinct nodes reachable from start_id within max_depth hops. Wraps impact() with COUNT(DISTINCT).';

-- ─────────────────────────────────────────────────────────────────────
-- path(p_start_id, p_end_id, p_max_depth, p_project_code)
--
-- Shortest path from start to end, up to max_depth hops. One row per
-- hop, empty if no path exists within depth. Powers the MCP `path` tool.
-- ─────────────────────────────────────────────────────────────────────
CREATE OR REPLACE FUNCTION public.path(
    p_start_id     TEXT,
    p_end_id       TEXT,
    p_max_depth    INT  DEFAULT 10,
    p_project_code TEXT DEFAULT ''
) RETURNS TABLE (
    hop           INT,
    node_id       TEXT,
    relation_type TEXT
)
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    WITH RECURSIVE walk AS (
        SELECT
            p_start_id          AS current_id,
            ARRAY[p_start_id]   AS path_nodes,
            ARRAY[]::TEXT[]     AS path_rels,
            0                   AS depth
        UNION ALL
        SELECT
            e.target_id,
            w.path_nodes || e.target_id,
            w.path_rels  || e.relation_type,
            w.depth + 1
        FROM walk w
        JOIN public.Edge e ON e.source_id = w.current_id
        WHERE w.depth < p_max_depth
          AND NOT (e.target_id = ANY(w.path_nodes))
          AND (p_project_code = '' OR e.project_code = p_project_code)
    ),
    shortest AS (
        SELECT path_nodes, path_rels, depth
        FROM walk
        WHERE current_id = p_end_id
        ORDER BY depth
        LIMIT 1
    )
    SELECT
        (ord.ord - 1)::INT  AS hop,
        ord.node_id         AS node_id,
        CASE
            WHEN ord.ord <= array_length(s.path_rels, 1) THEN s.path_rels[ord.ord]
            ELSE NULL
        END                 AS relation_type
    FROM shortest s,
         unnest(s.path_nodes) WITH ORDINALITY AS ord(node_id, ord)
    ORDER BY ord.ord;
$$;

COMMENT ON FUNCTION public.path(TEXT, TEXT, INT, TEXT) IS
'Shortest path from start_id to end_id on public.Edge, up to max_depth hops. One row per hop. Empty if unreachable.';

-- ─────────────────────────────────────────────────────────────────────
-- retrieve_context_v2(query_text, query_embedding, project_code, k)
--
-- Unified hybrid retrieval over three orthogonal indexes on Chunk
-- substance:
--   1. FTS lane    — public.Chunk.content_tsv GIN.
--   2. Vector lane — public.ChunkEmbedding.embedding HNSW (cosine).
--   3. Graph lane  — public.Edge 2-hop expansion around lanes 1 & 2 seeds.
--
-- The three candidate sets are fused with Reciprocal Rank Fusion
-- (Cormack et al. 2009, k_rrf = 60). RRF is robust to score-scale
-- differences across heterogeneous rankers. The whole computation lives
-- in a single PG plan so the planner can choose join order across lanes.
--
-- Acceptance target (VAL-AXO-073 gate 4): p95 < 100 ms on the AXO corpus.
-- ─────────────────────────────────────────────────────────────────────
CREATE OR REPLACE FUNCTION public.retrieve_context_v2(
    p_query_text      TEXT,
    p_query_embedding vector(1024),
    p_project_code    TEXT,
    p_k               INT DEFAULT 20
) RETURNS TABLE (
    chunk_id         TEXT,
    content          TEXT,
    file_path        TEXT,
    symbol_id        TEXT,
    rrf_score        DOUBLE PRECISION,
    fts_score        DOUBLE PRECISION,
    vector_distance  DOUBLE PRECISION,
    graph_distance   INT
)
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    WITH
    -- Lane 1 — FTS. plainto_tsquery normalises the free-text query;
    -- over-fetch 3x p_k so RRF has room to blend.
    fts_lane AS (
        SELECT
            c.id AS chunk_id,
            ROW_NUMBER() OVER (
                ORDER BY ts_rank_cd(c.content_tsv,
                                    plainto_tsquery('english', p_query_text)) DESC
            ) AS rank,
            ts_rank_cd(c.content_tsv,
                       plainto_tsquery('english', p_query_text)) AS score
        FROM public.Chunk c
        WHERE (p_project_code = '' OR c.project_code = p_project_code)
          AND c.content_tsv @@ plainto_tsquery('english', p_query_text)
        ORDER BY score DESC
        LIMIT GREATEST(p_k * 3, 30)
    ),
    -- Lane 2 — Vector ANN. HNSW index on (embedding vector_cosine_ops);
    -- `<=>` is pgvector's cosine-distance operator.
    vector_lane AS (
        SELECT
            ce.chunk_id,
            ROW_NUMBER() OVER (ORDER BY ce.embedding <=> p_query_embedding) AS rank,
            (ce.embedding <=> p_query_embedding) AS distance
        FROM public.ChunkEmbedding ce
        WHERE (p_project_code = '' OR ce.project_code = p_project_code)
        ORDER BY ce.embedding <=> p_query_embedding
        LIMIT GREATEST(p_k * 3, 30)
    ),
    -- Lane 3a — Seed symbols from the FTS/Vector candidates. These are
    -- the symbols whose 2-hop neighbourhood is worth pulling into the
    -- candidate set even if the symbol itself didn't match text or
    -- semantics.
    seed_symbols AS (
        SELECT DISTINCT c.source_id AS sym_id
        FROM public.Chunk c
        WHERE c.source_type = 'symbol'
          AND c.id IN (
              SELECT chunk_id FROM fts_lane
              UNION
              SELECT chunk_id FROM vector_lane
          )
    ),
    -- Lane 3b — Two-hop expansion on public.Edge. Depth bound to 2 so
    -- the join volume stays tractable. Cycle-safe by construction (each
    -- hop is a separate JOIN, not a recursive CTE).
    expanded_symbols AS (
        SELECT sym_id, 0 AS distance FROM seed_symbols
        UNION
        SELECT e.target_id, 1 AS distance
        FROM public.Edge e
        JOIN seed_symbols s ON e.source_id = s.sym_id
        WHERE p_project_code = '' OR e.project_code = p_project_code
        UNION
        SELECT e2.target_id, 2 AS distance
        FROM public.Edge e1
        JOIN seed_symbols s ON e1.source_id = s.sym_id
        JOIN public.Edge e2 ON e2.source_id = e1.target_id
        WHERE p_project_code = ''
           OR (e1.project_code = p_project_code AND e2.project_code = p_project_code)
    ),
    graph_lane AS (
        SELECT
            c.id AS chunk_id,
            ROW_NUMBER() OVER (ORDER BY MIN(es.distance), c.id) AS rank,
            MIN(es.distance)::INT AS distance
        FROM public.Chunk c
        JOIN expanded_symbols es ON c.source_id = es.sym_id
        WHERE c.source_type = 'symbol'
          AND (p_project_code = '' OR c.project_code = p_project_code)
        GROUP BY c.id
        ORDER BY MIN(es.distance), c.id
        LIMIT GREATEST(p_k * 3, 30)
    ),
    -- RRF fusion — contribution from each lane summed per chunk.
    -- k_rrf = 60 is the Cormack-Clarke-Buettcher constant.
    fused AS (
        SELECT chunk_id, 1.0 / (60.0 + rank) AS contrib FROM fts_lane
        UNION ALL
        SELECT chunk_id, 1.0 / (60.0 + rank) AS contrib FROM vector_lane
        UNION ALL
        SELECT chunk_id, 1.0 / (60.0 + rank) AS contrib FROM graph_lane
    ),
    ranked AS (
        SELECT chunk_id, SUM(contrib)::DOUBLE PRECISION AS rrf_score
        FROM fused
        GROUP BY chunk_id
        ORDER BY rrf_score DESC
        LIMIT p_k
    )
    SELECT
        r.chunk_id,
        c.content,
        c.file_path,
        c.source_id   AS symbol_id,
        r.rrf_score,
        COALESCE(f.score,    0.0)::DOUBLE PRECISION AS fts_score,
        COALESCE(v.distance, 1.0)::DOUBLE PRECISION AS vector_distance,
        COALESCE(g.distance, 999)::INT              AS graph_distance
    FROM ranked r
    LEFT JOIN public.Chunk c ON c.id = r.chunk_id
    LEFT JOIN fts_lane    f ON f.chunk_id = r.chunk_id
    LEFT JOIN vector_lane v ON v.chunk_id = r.chunk_id
    LEFT JOIN graph_lane  g ON g.chunk_id = r.chunk_id
    ORDER BY r.rrf_score DESC;
$$;

COMMENT ON FUNCTION public.retrieve_context_v2(TEXT, vector, TEXT, INT) IS
'Hybrid retrieval over Chunk substance: FTS (content_tsv) + vector ANN (pgvector cosine) + graph expansion (public.Edge depth-2). RRF k=60 fusion in one PG plan. VAL-AXO-073 target: p95 < 100ms.';
