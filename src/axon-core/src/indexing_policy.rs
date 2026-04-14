use crate::config::IndexingConfig;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EcosystemId {
    JavaScript,
    TypeScript,
    Python,
    Elixir,
    Erlang,
    Rust,
    Go,
    Jvm,
    C,
    Cpp,
    CSharp,
    Ruby,
    Php,
    WebAssets,
    DataLogic,
    General,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactClass {
    DependencyStore,
    BuildOutput,
    Cache,
    ToolingState,
    GeneratedFrameworkOutput,
    RuntimeArtifact,
    RepositoryMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExclusionPolicy {
    HardExclude,
    SoftExclude,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathDisposition {
    Allow,
    HardExcluded {
        ecosystem: EcosystemId,
        class: ArtifactClass,
        rule_id: &'static str,
    },
    SoftExcluded {
        ecosystem: EcosystemId,
        class: ArtifactClass,
        rule_id: &'static str,
    },
}

pub fn classify_path(
    root: &Path,
    path: &Path,
    config: &IndexingConfig,
    supported_ecosystems: &[EcosystemId],
) -> PathDisposition {
    classify_internal(
        root,
        path,
        config,
        supported_ecosystems,
        &config.ignored_directory_segments,
    )
}

pub fn classify_subtree_hint_path(
    root: &Path,
    path: &Path,
    config: &IndexingConfig,
    supported_ecosystems: &[EcosystemId],
) -> PathDisposition {
    let mut additional_hard_excludes = config.ignored_directory_segments.clone();
    additional_hard_excludes.extend(config.blocked_subtree_hint_segments.iter().cloned());

    classify_internal(
        root,
        path,
        config,
        supported_ecosystems,
        &additional_hard_excludes,
    )
}

struct DirectoryRule {
    ecosystem: EcosystemId,
    class: ArtifactClass,
    policy: ExclusionPolicy,
    rule_id: &'static str,
    matcher: DirectoryMatcher,
}

enum DirectoryMatcher {
    Exact(&'static str),
    Prefix(&'static str),
}

impl DirectoryMatcher {
    fn matches(&self, segment: &str) -> bool {
        match self {
            Self::Exact(value) => segment == *value,
            Self::Prefix(prefix) => segment.starts_with(*prefix),
        }
    }

    fn canonical_segment(&self) -> &'static str {
        match self {
            Self::Exact(value) => value,
            Self::Prefix(prefix) => prefix,
        }
    }
}

const DIRECTORY_RULES: &[DirectoryRule] = &[
    DirectoryRule {
        ecosystem: EcosystemId::General,
        class: ArtifactClass::RepositoryMetadata,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "repo_git_metadata",
        matcher: DirectoryMatcher::Exact(".git"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::General,
        class: ArtifactClass::RepositoryMetadata,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "repo_svn_metadata",
        matcher: DirectoryMatcher::Exact(".svn"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::General,
        class: ArtifactClass::RepositoryMetadata,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "repo_hg_metadata",
        matcher: DirectoryMatcher::Exact(".hg"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::General,
        class: ArtifactClass::ToolingState,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "env_direnv",
        matcher: DirectoryMatcher::Exact(".direnv"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::General,
        class: ArtifactClass::ToolingState,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "env_devenv",
        matcher: DirectoryMatcher::Exact(".devenv"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::General,
        class: ArtifactClass::Cache,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "cache_generic",
        matcher: DirectoryMatcher::Exact(".cache"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::JavaScript,
        class: ArtifactClass::DependencyStore,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "javascript_node_modules",
        matcher: DirectoryMatcher::Exact("node_modules"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::TypeScript,
        class: ArtifactClass::GeneratedFrameworkOutput,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "typescript_next_output",
        matcher: DirectoryMatcher::Exact(".next"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::TypeScript,
        class: ArtifactClass::GeneratedFrameworkOutput,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "typescript_nuxt_output",
        matcher: DirectoryMatcher::Exact(".nuxt"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::TypeScript,
        class: ArtifactClass::GeneratedFrameworkOutput,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "typescript_sveltekit_output",
        matcher: DirectoryMatcher::Exact(".svelte-kit"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::TypeScript,
        class: ArtifactClass::ToolingState,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "typescript_turbo_state",
        matcher: DirectoryMatcher::Exact(".turbo"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::TypeScript,
        class: ArtifactClass::Cache,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "typescript_parcel_cache",
        matcher: DirectoryMatcher::Exact(".parcel-cache"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Python,
        class: ArtifactClass::Cache,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "python_pycache",
        matcher: DirectoryMatcher::Exact("__pycache__"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Python,
        class: ArtifactClass::ToolingState,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "python_venv_dot",
        matcher: DirectoryMatcher::Exact(".venv"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Python,
        class: ArtifactClass::ToolingState,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "python_venv_plain",
        matcher: DirectoryMatcher::Exact("venv"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Python,
        class: ArtifactClass::Cache,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "python_pytest_cache",
        matcher: DirectoryMatcher::Exact(".pytest_cache"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Python,
        class: ArtifactClass::Cache,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "python_mypy_cache",
        matcher: DirectoryMatcher::Exact(".mypy_cache"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Python,
        class: ArtifactClass::Cache,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "python_ruff_cache",
        matcher: DirectoryMatcher::Exact(".ruff_cache"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Python,
        class: ArtifactClass::ToolingState,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "python_tox",
        matcher: DirectoryMatcher::Exact(".tox"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Python,
        class: ArtifactClass::ToolingState,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "python_nox",
        matcher: DirectoryMatcher::Exact(".nox"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Python,
        class: ArtifactClass::DependencyStore,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "python_eggs",
        matcher: DirectoryMatcher::Exact(".eggs"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Python,
        class: ArtifactClass::DependencyStore,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "python_site_packages",
        matcher: DirectoryMatcher::Exact("site-packages"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Python,
        class: ArtifactClass::DependencyStore,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "python_dist_packages",
        matcher: DirectoryMatcher::Exact("dist-packages"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Elixir,
        class: ArtifactClass::BuildOutput,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "elixir_build_prefix",
        matcher: DirectoryMatcher::Prefix("_build"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Elixir,
        class: ArtifactClass::DependencyStore,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "elixir_deps",
        matcher: DirectoryMatcher::Exact("deps"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Elixir,
        class: ArtifactClass::ToolingState,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "elixir_ls_state",
        matcher: DirectoryMatcher::Exact(".elixir_ls"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Elixir,
        class: ArtifactClass::ToolingState,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "elixir_mix_state",
        matcher: DirectoryMatcher::Exact(".mix"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Erlang,
        class: ArtifactClass::BuildOutput,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "erlang_ebin",
        matcher: DirectoryMatcher::Exact("ebin"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Erlang,
        class: ArtifactClass::ToolingState,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "erlang_rebar3",
        matcher: DirectoryMatcher::Exact(".rebar3"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Rust,
        class: ArtifactClass::BuildOutput,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "rust_target",
        matcher: DirectoryMatcher::Exact("target"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::Jvm,
        class: ArtifactClass::ToolingState,
        policy: ExclusionPolicy::HardExclude,
        rule_id: "jvm_gradle",
        matcher: DirectoryMatcher::Exact(".gradle"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::General,
        class: ArtifactClass::DependencyStore,
        policy: ExclusionPolicy::SoftExclude,
        rule_id: "shared_vendor",
        matcher: DirectoryMatcher::Exact("vendor"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::General,
        class: ArtifactClass::BuildOutput,
        policy: ExclusionPolicy::SoftExclude,
        rule_id: "shared_dist",
        matcher: DirectoryMatcher::Exact("dist"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::General,
        class: ArtifactClass::BuildOutput,
        policy: ExclusionPolicy::SoftExclude,
        rule_id: "shared_build",
        matcher: DirectoryMatcher::Exact("build"),
    },
    DirectoryRule {
        ecosystem: EcosystemId::General,
        class: ArtifactClass::BuildOutput,
        policy: ExclusionPolicy::SoftExclude,
        rule_id: "shared_out",
        matcher: DirectoryMatcher::Exact("out"),
    },
];

fn classify_internal(
    root: &Path,
    path: &Path,
    config: &IndexingConfig,
    supported_ecosystems: &[EcosystemId],
    additional_hard_excludes: &[String],
) -> PathDisposition {
    let relative = match relative_path_from_root(root, path) {
        Some(relative) => relative,
        None => {
            return PathDisposition::HardExcluded {
                ecosystem: EcosystemId::General,
                class: ArtifactClass::RepositoryMetadata,
                rule_id: "path_outside_root",
            }
        }
    };

    for segment in relative.iter().filter_map(|part| part.to_str()) {
        if segment_looks_like_embedded_windows_path(segment) {
            return PathDisposition::HardExcluded {
                ecosystem: EcosystemId::General,
                class: ArtifactClass::RepositoryMetadata,
                rule_id: "embedded_windows_foreign_path",
            };
        }

        if additional_hard_excludes
            .iter()
            .any(|value| value == segment)
        {
            return PathDisposition::HardExcluded {
                ecosystem: EcosystemId::General,
                class: ArtifactClass::ToolingState,
                rule_id: "config_hard_exclude_segment",
            };
        }

        if let Some(rule) = DIRECTORY_RULES
            .iter()
            .find(|rule| rule_applies(rule, segment, supported_ecosystems))
        {
            if matches!(rule.policy, ExclusionPolicy::SoftExclude)
                && config
                    .soft_excluded_directory_segments_allowlist
                    .iter()
                    .any(|allowed| {
                        allowed == segment || allowed == rule.matcher.canonical_segment()
                    })
            {
                continue;
            }

            return match rule.policy {
                ExclusionPolicy::HardExclude => PathDisposition::HardExcluded {
                    ecosystem: rule.ecosystem,
                    class: rule.class,
                    rule_id: rule.rule_id,
                },
                ExclusionPolicy::SoftExclude => PathDisposition::SoftExcluded {
                    ecosystem: rule.ecosystem,
                    class: rule.class,
                    rule_id: rule.rule_id,
                },
            };
        }
    }

    PathDisposition::Allow
}

fn relative_path_from_root(root: &Path, path: &Path) -> Option<PathBuf> {
    path.strip_prefix(root)
        .ok()
        .map(|relative| relative.to_path_buf())
}

fn segment_looks_like_embedded_windows_path(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn rule_applies(rule: &DirectoryRule, segment: &str, supported_ecosystems: &[EcosystemId]) -> bool {
    let ecosystem_is_active = matches!(rule.ecosystem, EcosystemId::General)
        || supported_ecosystems.contains(&rule.ecosystem);
    ecosystem_is_active && rule.matcher.matches(segment)
}

#[cfg(test)]
mod tests {
    use super::{
        classify_path, classify_subtree_hint_path, ArtifactClass, EcosystemId, PathDisposition,
    };
    use crate::config::IndexingConfig;
    use crate::parser::supported_parser_ecosystems;
    use std::path::Path;

    fn test_config() -> IndexingConfig {
        IndexingConfig {
            supported_extensions: vec!["rs".to_string(), "ts".to_string(), "py".to_string()],
            ignored_directory_segments: vec![],
            blocked_subtree_hint_segments: vec![],
            soft_excluded_directory_segments_allowlist: vec![],
            subtree_hint_cooldown_ms: 15_000,
            subtree_hint_retry_budget: 3,
            use_git_global_ignore: false,
            legacy_axonignore_additive: true,
            ignore_reconcile_enabled: true,
            ignore_reconcile_dry_run: true,
        }
    }

    #[test]
    fn test_supported_parser_ecosystems_expose_core_language_families() {
        let ecosystems = supported_parser_ecosystems();
        assert!(ecosystems.contains(&EcosystemId::JavaScript));
        assert!(ecosystems.contains(&EcosystemId::TypeScript));
        assert!(ecosystems.contains(&EcosystemId::Python));
        assert!(ecosystems.contains(&EcosystemId::Elixir));
        assert!(ecosystems.contains(&EcosystemId::Rust));
        assert!(ecosystems.contains(&EcosystemId::Go));
        assert!(ecosystems.contains(&EcosystemId::Jvm));
    }

    #[test]
    fn test_indexing_policy_hard_excludes_generated_build_prefixes_and_dependency_stores() {
        let config = test_config();
        let ecosystems = supported_parser_ecosystems();
        let root = Path::new("/workspace");

        assert_eq!(
            classify_path(
                root,
                Path::new("/workspace/proj/_build_truth_dashboard_ui/lib/app.ex"),
                &config,
                ecosystems,
            ),
            PathDisposition::HardExcluded {
                ecosystem: EcosystemId::Elixir,
                class: ArtifactClass::BuildOutput,
                rule_id: "elixir_build_prefix",
            }
        );

        assert_eq!(
            classify_path(
                root,
                Path::new("/workspace/proj/node_modules/react/index.js"),
                &config,
                ecosystems,
            ),
            PathDisposition::HardExcluded {
                ecosystem: EcosystemId::JavaScript,
                class: ArtifactClass::DependencyStore,
                rule_id: "javascript_node_modules",
            }
        );
    }

    #[test]
    fn test_indexing_policy_soft_excludes_vendor_without_blocking_normal_source_paths() {
        let config = test_config();
        let ecosystems = supported_parser_ecosystems();
        let root = Path::new("/workspace");

        assert_eq!(
            classify_path(
                root,
                Path::new("/workspace/proj/vendor/acme/lib.rb"),
                &config,
                ecosystems,
            ),
            PathDisposition::SoftExcluded {
                ecosystem: EcosystemId::General,
                class: ArtifactClass::DependencyStore,
                rule_id: "shared_vendor",
            }
        );

        assert_eq!(
            classify_path(
                root,
                Path::new("/workspace/proj/src/vendor_adapter.rs"),
                &config,
                ecosystems,
            ),
            PathDisposition::Allow
        );
    }

    #[test]
    fn test_indexing_policy_soft_exclude_allowlist_reopens_selected_segments_only() {
        let mut config = test_config();
        config.soft_excluded_directory_segments_allowlist = vec!["vendor".to_string()];
        let ecosystems = supported_parser_ecosystems();
        let root = Path::new("/workspace");

        assert_eq!(
            classify_path(
                root,
                Path::new("/workspace/proj/vendor/acme/lib.rb"),
                &config,
                ecosystems,
            ),
            PathDisposition::Allow
        );
        assert_eq!(
            classify_path(
                root,
                Path::new("/workspace/proj/node_modules/react/index.js"),
                &config,
                ecosystems,
            ),
            PathDisposition::HardExcluded {
                ecosystem: EcosystemId::JavaScript,
                class: ArtifactClass::DependencyStore,
                rule_id: "javascript_node_modules",
            }
        );
    }

    #[test]
    fn test_indexing_policy_subtree_hint_uses_additional_blocked_segments() {
        let mut config = test_config();
        config.blocked_subtree_hint_segments = vec!["pg_wal".to_string()];
        let ecosystems = supported_parser_ecosystems();
        let root = Path::new("/workspace");

        assert_eq!(
            classify_subtree_hint_path(
                root,
                Path::new("/workspace/proj/runtime/pg_wal"),
                &config,
                ecosystems,
            ),
            PathDisposition::HardExcluded {
                ecosystem: EcosystemId::General,
                class: ArtifactClass::ToolingState,
                rule_id: "config_hard_exclude_segment",
            }
        );
    }

    #[test]
    fn test_indexing_policy_subtree_hint_blocks_bmad_generated_scopes() {
        let mut config = test_config();
        config.blocked_subtree_hint_segments =
            vec!["_bmad".to_string(), "_bmad-output".to_string()];
        let ecosystems = supported_parser_ecosystems();
        let root = Path::new("/workspace");

        assert_eq!(
            classify_subtree_hint_path(
                root,
                Path::new("/workspace/proj/_bmad-output/planning-artifacts"),
                &config,
                ecosystems,
            ),
            PathDisposition::HardExcluded {
                ecosystem: EcosystemId::General,
                class: ArtifactClass::ToolingState,
                rule_id: "config_hard_exclude_segment",
            }
        );

        assert_eq!(
            classify_subtree_hint_path(
                root,
                Path::new("/workspace/proj/_bmad/_config"),
                &config,
                ecosystems,
            ),
            PathDisposition::HardExcluded {
                ecosystem: EcosystemId::General,
                class: ArtifactClass::ToolingState,
                rule_id: "config_hard_exclude_segment",
            }
        );
    }

    #[test]
    fn test_indexing_policy_hard_excludes_erlang_build_outputs_when_ecosystem_is_supported() {
        let config = test_config();
        let ecosystems = supported_parser_ecosystems();
        let root = Path::new("/workspace");

        assert_eq!(
            classify_path(
                root,
                Path::new("/workspace/proj/apps/demo/ebin/demo.app"),
                &config,
                ecosystems,
            ),
            PathDisposition::HardExcluded {
                ecosystem: EcosystemId::Erlang,
                class: ArtifactClass::BuildOutput,
                rule_id: "erlang_ebin",
            }
        );
    }

    #[test]
    fn test_indexing_policy_hard_excludes_embedded_windows_foreign_paths_inside_repo() {
        let config = test_config();
        let ecosystems = supported_parser_ecosystems();
        let root = Path::new("/workspace");

        assert_eq!(
            classify_subtree_hint_path(
                root,
                Path::new("/workspace/proj/C:\\Users\\dstad\\.claude\\plugins\\marketplaces\\claude-plugins-official"),
                &config,
                ecosystems,
            ),
            PathDisposition::HardExcluded {
                ecosystem: EcosystemId::General,
                class: ArtifactClass::RepositoryMetadata,
                rule_id: "embedded_windows_foreign_path",
            }
        );
    }
}
