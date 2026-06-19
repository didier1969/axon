use anyhow::anyhow;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use super::format::format_standard_contract;
use super::soll::{
    canonical_soll_export_dir, canonical_soll_site_dir, find_latest_soll_export, parse_soll_export,
    SollRestoreCounts,
};
use super::McpServer;
use crate::project_meta::{
    discover_project_identities, is_valid_project_code, resolve_canonical_project_identity,
};

mod completeness;
mod cycle_audit;
mod docs;
mod document_intent;
mod evidence;
mod inference;
mod manager;
mod methodology_bundle;
mod operations;
mod planning;
mod project_registry;
mod query_context;
mod relation_policy;
mod shared;
mod storage;
mod tech_debt;
mod workflow;

use inference::*;
use relation_policy::*;
use shared::*;
use storage::*;

#[allow(dead_code)]
const SOLL_RELATION_EXPORTS: [(&str, &str); 12] = [
    ("EPITOMIZES", "soll.EPITOMIZES"),
    ("BELONGS_TO", "soll.BELONGS_TO"),
    ("EXPLAINS", "soll.EXPLAINS"),
    ("SOLVES", "soll.SOLVES"),
    ("TARGETS", "soll.TARGETS"),
    ("VERIFIES", "soll.VERIFIES"),
    ("ORIGINATES", "soll.ORIGINATES"),
    ("SUPERSEDES", "soll.SUPERSEDES"),
    ("CONTRIBUTES_TO", "soll.CONTRIBUTES_TO"),
    ("REFINES", "soll.REFINES"),
    ("IMPACTS", "IMPACTS"),
    ("SUBSTANTIATES", "SUBSTANTIATES"),
];

#[allow(dead_code)]
type SollContextCache = HashMap<String, (i64, Value)>;

#[allow(dead_code)]
static SOLL_CONTEXT_CACHE: OnceLock<Mutex<SollContextCache>> = OnceLock::new();

#[allow(dead_code)]
const SOLL_CONTEXT_CACHE_TTL_MS: i64 = 180_000;
const SOLL_PROJECT_DOCS_GENERATOR_VERSION: &str = "soll_generate_docs_v3";
const SOLL_ROOT_DOCS_GENERATOR_VERSION: &str = "soll_generate_docs_root_v2";

impl McpServer {
    #[cfg(not(test))]
    fn soll_context_cache() -> &'static Mutex<SollContextCache> {
        SOLL_CONTEXT_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    #[cfg(not(test))]
    fn read_soll_context_cache(key: &str, now_ms: i64) -> Option<Value> {
        let guard = Self::soll_context_cache().lock().ok()?;
        let (stored_at, value) = guard.get(key)?;
        if now_ms.saturating_sub(*stored_at) > SOLL_CONTEXT_CACHE_TTL_MS {
            return None;
        }
        Some(value.clone())
    }

    #[cfg(test)]
    fn read_soll_context_cache(_key: &str, _now_ms: i64) -> Option<Value> {
        None
    }

    #[cfg(not(test))]
    fn write_soll_context_cache(key: String, now_ms: i64, value: &Value) {
        if let Ok(mut guard) = Self::soll_context_cache().lock() {
            guard.insert(key, (now_ms, value.clone()));
        }
    }

    #[cfg(test)]
    fn write_soll_context_cache(_key: String, _now_ms: i64, _value: &Value) {}
}
