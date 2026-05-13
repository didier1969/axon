//! Per-file `project_code` resolution for pipeline_v2 (DEC-AXO-081).
//!
//! The session-17 spawn_pipeline_a contract fixed `project_code: Arc<str>`
//! at construction time — one indexer = one project, file paths got
//! stamped with that one code. Live indexers watch `~/projects/*` with
//! multiple project codes, so DEC-AXO-081 ratifies a resolver-based
//! signature: each file's project_code is computed at stage-A3 entry
//! (and at B3 by parsing the chunk_id prefix, which already carries the
//! project_code).

use std::path::Path;
use std::sync::Arc;

/// Callable that maps an arbitrary watch-root-relative or absolute file
/// path to its 3-letter project code (e.g. `AXO`, `RMC`, `DOC`).
///
/// Implementations are expected to be cheap (Scanner does an in-memory
/// project-meta lookup; the const variant is a fixed string clone).
pub type ProjectCodeResolver = Arc<dyn Fn(&Path) -> String + Send + Sync>;

/// Build a resolver that always returns the same project code. Used by
/// the bench harness + tests where the file set is single-project by
/// construction.
pub fn const_resolver(project_code: impl Into<String>) -> ProjectCodeResolver {
    let code = project_code.into();
    Arc::new(move |_path: &Path| code.clone())
}

/// Extract the project_code from a v2 chunk_id (canonical format
/// `"{project_code}::{path_namespace}::{name}::chunk[::part-NN]"`).
///
/// Returns `None` when the chunk_id is malformed (no `::` delimiter).
/// B3 uses this so it can stamp `public.ChunkEmbedding.project_code`
/// without having to thread the code through B1/B2.
pub fn project_code_from_chunk_id(chunk_id: &str) -> Option<&str> {
    chunk_id.split("::").next().filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn const_resolver_returns_supplied_code_for_any_path() {
        let r = const_resolver("AXO");
        assert_eq!(r(Path::new("/anywhere/foo.rs")), "AXO");
        assert_eq!(r(Path::new("/elsewhere/bar.ex")), "AXO");
    }

    #[test]
    fn project_code_from_chunk_id_parses_canonical_format() {
        assert_eq!(
            project_code_from_chunk_id("AXO::src__main_rs::main::chunk"),
            Some("AXO")
        );
        assert_eq!(
            project_code_from_chunk_id("RMC::lib__util_rs::helper::chunk::part-03"),
            Some("RMC")
        );
    }

    #[test]
    fn project_code_from_chunk_id_returns_none_on_malformed_input() {
        assert_eq!(project_code_from_chunk_id(""), None);
        assert_eq!(project_code_from_chunk_id("no_delimiter_at_all"), Some("no_delimiter_at_all"));
        // Empty leading segment is rejected — a stray `::name` is not a
        // valid project_code prefix.
        assert_eq!(project_code_from_chunk_id("::bad::chunk"), None);
    }
}
