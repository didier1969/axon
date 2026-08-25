// MIL-AXO-015 P2 (REQ-AXO-208): Per-project schema namespace generator.
//
// Two surfaces:
//   - `generate_global_schema()`: idempotent DDL for the public + soll
//     schemas (extensions, ProjectCodeRegistry, SOLL Node/Edge/Revision/
//     Traceability). Run once at deployment bootstrap.
//   - `generate_project_schema(project_code)`: idempotent DDL for one
//     project's IST namespace (File, Symbol, Chunk, ChunkEmbedding with
//     pgvector, CONTAINS/CALLS/etc. relations, queues, telemetry, AGE
//     graph). Run by axon_init_project (P5) when registering a new
//     project.
//
// Architecture references:
//   - DEC-AXO-075: PG replaces DuckDB.
//   - CPT-AXO-039: per-project schema namespace.
//   - CPT-AXO-040: Apache AGE for graph queries.
//   - CPT-AXO-041: pgvector HNSW for ChunkEmbedding.
//
// Idempotence is the design constraint: every statement uses
// IF NOT EXISTS / IF EXISTS / OR REPLACE so re-running on a healthy
// database is a no-op. P3 will exercise these against a real PG via
// testcontainers; P2 only proves DDL stability.

use anyhow::{anyhow, Result};

/// Validate a project_code so it can be used as a PostgreSQL schema
/// identifier without quoting. Axon uses 3-letter uppercase codes (AXO,
/// FSF, etc.) but the schema namespace is lowercased to match Postgres
/// case-folding rules. We refuse anything that isn't strictly alphanum
/// + underscore so generated SQL is injection-free even if a malicious
/// caller bypasses the registry layer.
pub fn schema_name_for(project_code: &str) -> Result<String> {
    if project_code.is_empty() {
        return Err(anyhow!("project_code is empty"));
    }
    if project_code.len() > 32 {
        return Err(anyhow!(
            "project_code '{}' too long (>32 chars)",
            project_code
        ));
    }
    if !project_code
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(anyhow!(
            "project_code '{}' contains characters that are not [a-zA-Z0-9_]",
            project_code
        ));
    }
    Ok(project_code.to_ascii_lowercase())
}

/// Global DDL: extensions + public registry + soll intent layer + IST
/// multi-project tables (post-CPT-AXO-039 supersedure 2026-05-08) +
/// axon indexer telemetry. Stable, byte-identical across calls
/// for the same Axon binary build.
///
/// REQ-AXO-91545 follow-up (MIL-AXO-020 closure): the canonical DDL
/// lives in `db/ddl/*.sql` and is compiled into the binary via
/// `include_str!` in `load_canonical_ddl_files`. This function is the
/// thin wrapper that preserves the API for `bootstrap_global_pg_schema`
/// in `graph_bootstrap.rs`.
pub fn generate_global_schema() -> Vec<String> {
    load_canonical_ddl_files()
}

// REQ-AXO-902328 — la liste des fichiers DDL compilés est DÉRIVÉE du répertoire
// `db/ddl/` par `build.rs`, plus jamais écrite à la main.
//
// Elle l'était, et il lui manquait **9 fichiers sur 25** — tout le mailbox
// cross-projet, le schéma `axon` des pratiques, les contrats, les secrets projet.
// Le brain rejoue cette liste à CHAQUE boot ; les deux autres applicateurs
// (`scripts/lib/ensure-runtime.sh::apply_canonical_ddl`, au start ET au step 5b du
// promote) parcourent le répertoire. Un brain démarré sans le chemin shell ne
// créait donc aucune des tables de ces 9 fichiers.
//
// Le trou était ÉCRIT, pas distrait : « the other 14..22 files are unrelated » —
// vrai pour le commit qui l'a posé (un NOTIFY sur soll.Traceability), faux pour le
// bootstrap. Une liste manuelle invite ce raisonnement local sur une décision
// globale ; un répertoire l'interdit. C'est GUI-PRO-013 au chokepoint.
//
// Définit `const CANONICAL_DDL_FILES: &[(&str, &str)]` — (nom de fichier, contenu).
include!(concat!(env!("OUT_DIR"), "/canonical_ddl_files.rs"));



/// REQ-AXO-902328 — les NOMS des fichiers DDL canoniques, dans l'ordre d'application.
///
/// Existe pour que le harnais de test applique EXACTEMENT ce que le brain compile,
/// au lieu de re-parcourir `db/ddl/` avec sa propre règle. Il y avait trois règles
/// de sélection pour une seule question (« quel est le schéma canonique ? ») ; c'est
/// justement parce que les tests en avaient une troisième, plus complète, qu'ils
/// n'ont jamais vu les 9 fichiers manquants au brain.
// Consommé par le seul harnais de test : en production c'est le CONTENU qui sert
// (`load_canonical_ddl_files`), pas les noms. Le gate dit cette portée au lieu de
// laisser un `#[allow(dead_code)]` la taire.
#[cfg(test)]
pub(crate) fn canonical_ddl_file_names() -> Vec<&'static str> {
    CANONICAL_DDL_FILES.iter().map(|(nom, _)| *nom).collect()
}

/// MIL-AXO-020 — canonical DDL files compiled into the binary via
/// `include_str!`. Each file is split into top-level statements,
/// respecting `$tag$ … $tag$` dollar-quoted regions used by PL/pgSQL
/// function bodies and DO blocks. The split is whitespace-trimmed; an
/// empty trailing statement is silently dropped.
///
/// File order matches numeric prefix (00 → 05) so dependencies resolve
/// in the same order as `./scripts/start.sh` applies them at runtime.
pub fn load_canonical_ddl_files() -> Vec<String> {
    let mut stmts = Vec::new();
    for (_name, body) in CANONICAL_DDL_FILES {
        stmts.extend(split_top_level_statements(body));
    }
    stmts
}

/// Split a multi-statement SQL script on top-level `;` while respecting
/// PostgreSQL dollar-quoted regions (`$tag$…$tag$`, `$$…$$`) and single-
/// quoted strings (with `''` escape). Line comments (`--`) and block
/// comments (`/* … */`) inside the SQL are preserved verbatim — they
/// just don't get split apart from their surrounding statement.
pub(crate) fn split_top_level_statements(input: &str) -> Vec<String> {
    let mut stmts = Vec::new();
    let mut current = String::new();
    let bytes: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut in_single_quote = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut dollar_tag: Option<String> = None;

    while i < bytes.len() {
        let c = bytes[i];

        if in_line_comment {
            current.push(c);
            if c == '\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            current.push(c);
            if c == '*' && i + 1 < bytes.len() && bytes[i + 1] == '/' {
                current.push(bytes[i + 1]);
                i += 2;
                in_block_comment = false;
                continue;
            }
            i += 1;
            continue;
        }
        if let Some(tag) = &dollar_tag {
            current.push(c);
            if c == '$' {
                let needed: Vec<char> = tag.chars().chain(std::iter::once('$')).collect();
                if i + needed.len() < bytes.len() && bytes[i + 1..=i + needed.len()] == needed[..] {
                    for j in 1..=needed.len() {
                        current.push(bytes[i + j]);
                    }
                    i += needed.len() + 1;
                    dollar_tag = None;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        if in_single_quote {
            current.push(c);
            if c == '\'' {
                if i + 1 < bytes.len() && bytes[i + 1] == '\'' {
                    current.push(bytes[i + 1]);
                    i += 2;
                    continue;
                }
                in_single_quote = false;
            }
            i += 1;
            continue;
        }

        // Not inside any quoted / commented region.
        if c == '-' && i + 1 < bytes.len() && bytes[i + 1] == '-' {
            in_line_comment = true;
            current.push(c);
            current.push(bytes[i + 1]);
            i += 2;
            continue;
        }
        if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == '*' {
            in_block_comment = true;
            current.push(c);
            current.push(bytes[i + 1]);
            i += 2;
            continue;
        }
        if c == '\'' {
            in_single_quote = true;
            current.push(c);
            i += 1;
            continue;
        }
        if c == '$' {
            // Detect $tag$ where tag matches [A-Za-z_][A-Za-z0-9_]* or is empty.
            let mut j = i + 1;
            while j < bytes.len() {
                let nc = bytes[j];
                if nc == '$' {
                    let tag: String = bytes[i + 1..j].iter().collect();
                    for k in i..=j {
                        current.push(bytes[k]);
                    }
                    i = j + 1;
                    dollar_tag = Some(tag);
                    break;
                }
                if !(nc.is_ascii_alphanumeric() || nc == '_') {
                    // Not a dollar-quote start; treat the leading `$` as a literal.
                    current.push(c);
                    i += 1;
                    break;
                }
                j += 1;
            }
            if dollar_tag.is_none() && i < bytes.len() && bytes[i] == c {
                // Loop exited without consuming — fall back to literal `$`.
                current.push(c);
                i += 1;
            }
            continue;
        }
        if c == ';' {
            let stmt = current.trim().to_string();
            if !stmt.is_empty() {
                stmts.push(stmt);
            }
            current.clear();
            i += 1;
            continue;
        }
        current.push(c);
        i += 1;
    }

    let trailing = current.trim().to_string();
    if !trailing.is_empty() {
        stmts.push(trailing);
    }
    stmts
}

/// Per-project provisioning entry point.
///
/// Pre-supersedure (CPT-AXO-039 era) this function created a dedicated
/// PG schema per project. Post-supersedure (2026-05-08) it's a thin
/// pass-through that just validates the project_code and returns an
/// empty plan: every IST table now lives in `public` with a
/// `project_code` column, provisioned once by `generate_global_schema`.
/// We keep the function for API stability — `axon_init_project` still
/// calls it, and it still rejects malformed codes (SQL-injection guard
/// applies even if no DDL fires).
pub fn generate_project_schema(project_code: &str) -> Result<Vec<String>> {
    // Validate the project_code shape — same guard as before so
    // callers get the same error semantics on bad input.
    let _ = schema_name_for(project_code)?;
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_name_lowercases_and_validates() {
        assert_eq!(schema_name_for("AXO").unwrap(), "axo");
        assert_eq!(schema_name_for("FSF").unwrap(), "fsf");
        assert_eq!(schema_name_for("my_project").unwrap(), "my_project");
        assert_eq!(schema_name_for("Project_42").unwrap(), "project_42");
    }

    #[test]
    fn schema_name_rejects_injection_attempts() {
        assert!(schema_name_for("").is_err());
        assert!(schema_name_for("axo; DROP TABLE Node;--").is_err());
        assert!(schema_name_for("axo--").is_err());
        assert!(schema_name_for("axo;").is_err());
        assert!(schema_name_for("axo space").is_err());
        assert!(schema_name_for("axo'").is_err());
    }

    #[test]
    fn schema_name_rejects_overlong() {
        let long = "a".repeat(33);
        assert!(schema_name_for(&long).is_err());
        let max = "a".repeat(32);
        assert!(schema_name_for(&max).is_ok());
    }

    #[test]
    fn global_schema_is_byte_stable_across_calls() {
        let a = generate_global_schema();
        let b = generate_global_schema();
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }

    #[test]
    fn project_schema_is_now_no_op() {
        // CPT-AXO-039 superseded 2026-05-08: per-project schema replaced
        // by multi-project tables in `public`. The function still
        // validates project_code shape but emits zero DDL statements.
        let stmts = generate_project_schema("AXO").unwrap();
        assert!(
            stmts.is_empty(),
            "generate_project_schema should be a no-op post-CPT-AXO-039 supersedure"
        );
    }

    #[test]
    fn global_schema_includes_required_objects() {
        let stmts = generate_global_schema();
        let joined = stmts.join("\n");
        // MIL-AXO-017 slice 6B Phase E: AGE retired ; pgvector + pg_trgm now canonical.
        assert!(joined.contains("CREATE EXTENSION IF NOT EXISTS vector"));
        assert!(joined.contains("CREATE SCHEMA IF NOT EXISTS soll"));
        // REQ-AXO-247: ProjectCodeRegistry now lives in `soll`, not
        // `public`, so the consumer code path (axon_init_project,
        // soll_validate, axon_commit_work) finds it under PG.
        assert!(joined.contains("soll.ProjectCodeRegistry"));
        assert!(
            !joined.contains("public.ProjectCodeRegistry"),
            "PCR should no longer be in public; consumers query soll.*"
        );
        // REQ-AXO-901881 — `soll.ProjectCodeRegistry.project_code` is the PRIMARY
        // KEY, whose implicit unique index already covers lookups by code. The
        // canonical DDL (01_soll_schema.sql) deliberately emits NO separate
        // `soll_project_code_registry_code_idx` (audited + EXPLAIN-proven, see the
        // "── Indexes ──" comment). Assert the PK contract instead of a redundant
        // index the schema intentionally omits (the old assertion was stale).
        // Whitespace-collapsed so a future column re-alignment in the DDL cannot
        // re-break this on cosmetic spacing.
        let joined_ws = joined.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            joined_ws.contains("project_code TEXT PRIMARY KEY"),
            "PCR.project_code must be the PRIMARY KEY (its implicit index covers code lookups)"
        );
        for tbl in [
            "soll.Registry",
            "soll.Node",
            "soll.Edge",
            "soll.Revision",
            "soll.RevisionChange",
            "soll.RevisionPreview",
            "soll.Traceability",
            "soll.McpJob",
        ] {
            assert!(
                joined.contains(tbl),
                "expected SOLL schema to contain {tbl}"
            );
        }
    }

    // MIL-AXO-017 slice 6B Phase D: AGE label tests removed with the helper.

    #[test]
    fn global_schema_includes_multi_project_ist_tables() {
        // REQ-AXO-901653 slice-5c — public.File + public.GraphProjectionQueue
        // dropped from the canonical schema ; assertion list updated to match.
        let joined = generate_global_schema().join("\n");
        for tbl in [
            "ist.IndexedFile",
            "ist.Symbol",
            "ist.Chunk",
            "ist.ChunkEmbedding",
            "ist.FileLifecycleEvent",
            "ist.HourlyVectorizationRollup",
            "axon.Project",
            "axon.EmbeddingModel",
            "ist.GraphProjection",
            "ist.GraphProjectionState",
            "ist.GraphEmbedding",
        ] {
            assert!(
                joined.contains(tbl),
                "expected IST table {tbl} in global schema"
            );
        }
        // ChunkEmbedding gains project_code column for multi-project
        // filtering under the single global HNSW index.
        assert!(
            joined.contains("ist.ChunkEmbedding") && joined.contains("project_code TEXT NOT NULL")
        );
        // Single global HNSW index (CPT-AXO-041). Substrings rather than
        // a single contiguous match because the canonical SQL formats
        // the statement across multiple lines.
        assert!(joined.contains("chunk_embedding_hnsw_idx"));
        assert!(joined.contains("ist.ChunkEmbedding"));
        assert!(joined.contains("USING hnsw"));
        assert!(joined.contains("vector_cosine_ops"));
        // create_graph assertion retired (MIL-AXO-017 Phase E).
        // No per-project schema artefacts left (with word boundaries
        // so `axon` doesn't trigger the false-positive).
        assert!(!joined.contains("CREATE SCHEMA IF NOT EXISTS axo "));
        assert!(!joined.contains("CREATE SCHEMA IF NOT EXISTS axo\n"));
        assert!(!joined.contains("axo.File"));
        assert!(!joined.contains("axo.Chunk"));
    }

    #[test]
    fn global_schema_includes_axon_tables() {
        // MIL-AXO-015 P4 4e seed: indexer hot-path tables must exist in PG.
        let joined = generate_global_schema().join("\n");
        assert!(joined.contains("CREATE SCHEMA IF NOT EXISTS axon"));
        for tbl in [
            "axon.VectorWorkerFault",
            "axon.VectorLaneState",
            "axon.VectorPersistOutbox",
            "axon.vector_batch_run",
        ] {
            assert!(
                joined.contains(tbl),
                "expected axon schema to contain {tbl}"
            );
        }
        // Idempotence: every CREATE TABLE statement (line-leading, not a
        // substring inside a `--` comment) uses IF NOT EXISTS. The
        // canonical SQL files keep explanatory `-- the CREATE TABLE
        // above is a no-op …` comments, so substring counts overcount
        // — gate at line level instead.
        let create_table_count = joined
            .lines()
            .filter(|l| l.trim_start().starts_with("CREATE TABLE"))
            .count();
        let if_not_exists_count = joined
            .lines()
            .filter(|l| l.trim_start().starts_with("CREATE TABLE IF NOT EXISTS"))
            .count();
        assert_eq!(
            create_table_count, if_not_exists_count,
            "all CREATE TABLE statements must be IF NOT EXISTS for idempotence"
        );
    }

    #[test]
    fn project_schema_validates_input() {
        // CPT-AXO-039 superseded but the validation remains: callers
        // pass project_code through schema_name_for to reject injection
        // attempts, even though no DDL is emitted.
        assert!(generate_project_schema("axo;DROP TABLE Node").is_err());
        assert!(generate_project_schema("").is_err());
        assert!(generate_project_schema("AXO").is_ok());
    }

    /// REQ-AXO-902328 — la liste compilée doit être le répertoire, pas un souvenir.
    ///
    /// Le dépôt applique le DDL canonique par TROIS chemins, et un seul choisissait
    /// ses fichiers à la main :
    ///
    /// | chemin | règle | appliquait |
    /// |---|---|---|
    /// | `scripts/lib/ensure-runtime.sh::apply_canonical_ddl` (start **et** promote 5b) | glob `[0-9][0-9]_*.sql` | 25 / 25 |
    /// | `bootstrap_global_pg_schema` (brain, à chaque boot) | cette liste | **16 / 25** |
    /// | `test_support::test_db::apply_sql_dir` | `read_dir` | 25 / 25 |
    ///
    /// Les 9 absents étaient `14_ist_data_artifacts`, `15/18/19/20_mailbox*`,
    /// `16_practice`, `17_agent_card`, `21_contract`, `22_project_secret` — soit
    /// le schéma `axon` des pratiques et tout le mailbox cross-projet, c'est-à-dire
    /// les surfaces que `CLAUDE.md` désigne comme canaux PRIMAIRES. Un brain démarré
    /// sans le chemin shell ne créait aucune de leurs tables.
    ///
    /// Le trou n'était pas un oubli : il était **écrit** (« *the other 14..22 files
    /// are unrelated (IST/mailbox/etc.)* »), et ce jugement était juste pour le
    /// commit qui l'a posé et faux pour le bootstrap. C'est la liste manuelle qui
    /// rend ce glissement possible ; un répertoire ne le permet pas.
    ///
    /// ⚠️ **Ce que cette garde vaut, dit franchement.** Depuis que `build.rs` dérive
    /// la liste du répertoire, elle compare deux dérivés d'une même source : elle est
    /// proche de la tautologie, et le vrai anti-régression est
    /// `cargo:rerun-if-changed=db/ddl`. Sa valeur a été d'être le **témoin de
    /// falsification** — rouge en nommant les 9 avant le correctif — et cette valeur
    /// se dépense une fois. Elle reste parce qu'elle attrape encore un cas réel :
    /// un binaire compilé sur un arbre différent de celui qu'on teste, exactement ce
    /// qu'a servi le promote du 2026-08-23.
    #[test]
    fn the_compiled_ddl_list_matches_the_directory() {
        let ddl_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("db")
            .join("ddl");

        // La règle du SHELL, à l'identique : `[0-9][0-9]_*.sql` — deux chiffres,
        // un souligné, puis n'importe quoi. C'est elle qui a réellement appliqué
        // les 25 fichiers en live ; c'est donc elle qui fait autorité.
        let mut sur_disque: Vec<String> = std::fs::read_dir(&ddl_dir)
            .unwrap_or_else(|e| panic!("db/ddl illisible ({}): {e}", ddl_dir.display()))
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .filter(|nom| {
                let o = nom.as_bytes();
                nom.ends_with(".sql")
                    && o.len() > 3
                    && o[0].is_ascii_digit()
                    && o[1].is_ascii_digit()
                    && o[2] == b'_'
            })
            .collect();
        sur_disque.sort();

        let mut compiles: Vec<String> = CANONICAL_DDL_FILES
            .iter()
            .map(|(nom, _)| (*nom).to_owned())
            .collect();
        compiles.sort();

        let manquants: Vec<&String> = sur_disque.iter().filter(|n| !compiles.contains(n)).collect();
        let fantomes: Vec<&String> = compiles.iter().filter(|n| !sur_disque.contains(n)).collect();

        assert!(
            manquants.is_empty() && fantomes.is_empty(),
            "le DDL compilé dans le binaire diverge de `db/ddl/` — {} sur disque, {} compilé(s).\n\
             \x20 JAMAIS appliqué par le brain ({}) :\n   {}\n\
             \x20 compilé mais absent du disque ({}) :\n   {}\n\
             Le brain applique cette liste à CHAQUE boot ; le shell applique le répertoire. \
             Un fichier qui n'est que sur le disque n'existe pour le brain d'aucune façon — \
             ses tables manquent sur tout déploiement qui ne passe pas par \
             `scripts/lib/ensure-runtime.sh`. Ne pas rattraper à la main : la liste est \
             dérivée du répertoire par `build.rs` (REQ-AXO-902328).",
            sur_disque.len(),
            compiles.len(),
            manquants.len(),
            manquants
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n   "),
            fantomes.len(),
            fantomes
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n   "),
        );
    }

    /// REQ-AXO-902328 — contrôle POSITIF : le bootstrap du brain porte enfin les
    /// surfaces que `CLAUDE.md` désigne comme canaux PRIMAIRES.
    ///
    /// La garde de non-divergence compare des noms de fichiers ; elle ne dit rien
    /// de ce qu'ils CONTIENNENT. Ce test-ci vérifie la conséquence qui comptait :
    /// un brain qui boote crée le schéma `axon` des pratiques, les tables du
    /// mailbox cross-projet, les contrats. Avant le correctif, ces trois familles
    /// n'existaient que si l'on passait par `scripts/lib/ensure-runtime.sh`.
    #[test]
    fn the_brain_bootstrap_now_carries_practice_mailbox_and_contract() {
        let joined = generate_global_schema().join("\n");
        for objet in [
            "axon.practice",
            "axon.mailbox_message",
            "axon.mailbox_lease",
            "axon.agent_card",
            "soll.Contract",
            "axon.project_secret",
        ] {
            assert!(
                joined.to_lowercase().contains(&objet.to_lowercase()),
                "le DDL compilé dans le brain ne porte pas `{objet}` — un déploiement \
                 qui ne passe pas par ensure-runtime.sh n'aurait pas cette table \
                 (REQ-AXO-902328)"
            );
        }
    }
}
