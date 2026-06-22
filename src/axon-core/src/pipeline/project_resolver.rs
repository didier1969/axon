//! Per-file `project_code` resolution for pipeline (DEC-AXO-081).
//!
//! The session-17 spawn_pipeline_a contract fixed `project_code: Arc<str>`
//! at construction time — one indexer = one project, file paths got
//! stamped with that one code. Live indexers watch `~/projects/*` with
//! multiple project codes, so DEC-AXO-081 ratifies a resolver-based
//! signature: each file's project_code is computed at stage-A3 entry
//! (and at B3 by parsing the chunk_id prefix, which already carries the
//! project_code).

use std::path::{Path, PathBuf};
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
/// B3 uses this so it can stamp `ist.ChunkEmbedding.project_code`
/// without having to thread the code through B1/B2.
pub fn project_code_from_chunk_id(chunk_id: &str) -> Option<&str> {
    chunk_id.split("::").next().filter(|s| !s.is_empty())
}

/// Boot-time snapshot of the canonical project registry — resolves a file path
/// → project_code by LONGEST-PREFIX match, fully in RAM (PIL-AXO-007 CP2c).
///
/// Replaces the per-file filesystem rescan of every `.axon/meta.json` the old
/// resolver did on each A3 call (O(projects) disk I/O per file). Hydrated ONCE
/// at boot from PG (`soll.ProjectCodeRegistry`) — PIL-AXO-001: the PG registry
/// is the canonical source of truth, not the filesystem.
pub struct ProjectRegistrySnapshot {
    /// (canonical project_path, project_code), sorted by path length DESC so the
    /// first prefix match is the deepest (most specific) project.
    entries: Vec<(PathBuf, String)>,
}

impl ProjectRegistrySnapshot {
    /// Build from `(project_code, project_path)` rows. Empty codes/paths and the
    /// reserved cross-tenant `PRO` methodology code are dropped (they map to no
    /// IST project). Sorted by path length DESC for longest-prefix resolution.
    pub fn from_rows<I>(rows: I) -> Self
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut entries: Vec<(PathBuf, String)> = rows
            .into_iter()
            .filter(|(code, path)| !code.is_empty() && code != "PRO" && !path.is_empty())
            .map(|(code, path)| (PathBuf::from(path), code))
            .collect();
        entries.sort_by(|a, b| b.0.as_os_str().len().cmp(&a.0.as_os_str().len()));
        Self { entries }
    }

    /// Longest-prefix resolve: the code of the deepest registered project whose
    /// path is a path-component prefix of `path`, or `None` when `path` is
    /// outside every registered project. `Path::starts_with` is component-wise,
    /// so `/p/ab` never matches project `/p/a`.
    pub fn resolve(&self, path: &Path) -> Option<&str> {
        self.entries
            .iter()
            .find(|(proj_path, _)| path.starts_with(proj_path))
            .map(|(_, code)| code.as_str())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Wrap into a [`ProjectCodeResolver`] closure. Unresolved paths return the
    /// `"UNK"` sentinel — a DROP marker (A3 / graph_ingestion skip `"UNK"`,
    /// REQ-AXO-901860), matching the prior resolver's contract exactly.
    pub fn into_resolver(self) -> ProjectCodeResolver {
        let snap = Arc::new(self);
        Arc::new(move |path: &Path| {
            snap.resolve(path)
                .map(str::to_string)
                .unwrap_or_else(|| "UNK".to_string())
        })
    }
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
        assert_eq!(
            project_code_from_chunk_id("no_delimiter_at_all"),
            Some("no_delimiter_at_all")
        );
        // Empty leading segment is rejected — a stray `::name` is not a
        // valid project_code prefix.
        assert_eq!(project_code_from_chunk_id("::bad::chunk"), None);
    }

    // --- ProjectRegistrySnapshot (CP2c — RAM longest-prefix resolver) ---

    #[test]
    fn snapshot_resolves_longest_prefix_to_deepest_project() {
        let snap = ProjectRegistrySnapshot::from_rows([
            ("AAA".into(), "/home/u/projects/a".into()),
            ("BBB".into(), "/home/u/projects/a/b".into()),
        ]);
        // The deepest (most specific) registered project wins.
        assert_eq!(
            snap.resolve(Path::new("/home/u/projects/a/b/src/f.rs")),
            Some("BBB")
        );
        assert_eq!(
            snap.resolve(Path::new("/home/u/projects/a/x.rs")),
            Some("AAA")
        );
    }

    #[test]
    fn snapshot_returns_none_outside_all_projects() {
        let snap =
            ProjectRegistrySnapshot::from_rows([("AAA".into(), "/home/u/projects/a".into())]);
        assert_eq!(snap.resolve(Path::new("/tmp/elsewhere.rs")), None);
        // Component-wise: a sibling sharing a string prefix must NOT match.
        assert_eq!(snap.resolve(Path::new("/home/u/projects/ab/f.rs")), None);
    }

    #[test]
    fn snapshot_drops_empty_and_reserved_pro_codes() {
        let snap = ProjectRegistrySnapshot::from_rows([
            ("".into(), "/x".into()),
            ("PRO".into(), "/y".into()),
            ("AXO".into(), "/home/u/projects/axon".into()),
        ]);
        assert_eq!(snap.len(), 1);
        assert_eq!(
            snap.resolve(Path::new("/home/u/projects/axon/src/f.rs")),
            Some("AXO")
        );
    }

    #[test]
    fn snapshot_into_resolver_returns_unk_sentinel_for_unmatched() {
        let r =
            ProjectRegistrySnapshot::from_rows([("AXO".into(), "/home/u/projects/axon".into())])
                .into_resolver();
        assert_eq!(r(Path::new("/home/u/projects/axon/x.rs")), "AXO");
        assert_eq!(r(Path::new("/tmp/foo.rs")), "UNK");
    }
}
