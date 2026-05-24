---
status: draft (awaiting MCP-up to write to SOLL via soll_manager)
type: requirement
proposed_id: REQ-AXO-901737
links:
  refines: REQ-AXO-184  (single canonical knob — partiellement livré, fuites persistent)
  refines: DEC-AXO-070  (kill list item #6 — collapse multi-knob redundancy)
  parent: CPT-AXO-029   (IST freshness gate — runtime config doit être trustable)
tags: [simplifiable, robustness, deliverability, llm-friction, gpu-cpu-recurrence]
originator: incident SSD saturation 2026-05-23 + diagnostic CPU-silent-fallback (heartbeat live indexer 20:46 : requested=cpu effective=cpu alors que GPU disponible)
priority: P0
---

# REQ-AXO-901737 — Embedder provider : source unique de vérité (migration atomique)

## Problème (récurrent — 3ème fois ce semestre)
Le choix CPU vs GPU pour l'embedder est encore **distribué entre 11 variables env + 3 flags CLI + 4 logiques de défaut + 2 mécanismes de coercion**. Malgré REQ-AXO-184 (qui a supprimé une coercion), des fuites persistent. Conséquence ce soir : live indexer effectivement en CPU sans visibilité opérateur → vectorisation 20× plus lente → queue de chunks gonfle → RAM saturée → freeze WSL.

## Inventaire complet des sites de définition / lecture

### A. Variables d'environnement (11 total, à collapsr en 1)
| Variable | Rôle actuel | Sites de définition | Sites de lecture |
|---|---|---|---|
| `AXON_EMBEDDING_PROVIDER` | Demande opérateur (cpu/cuda/tensorrt) | `start.sh:181,432,462,510,598`, `runtime_boot.rs:518`, `embedder.rs:*` (tests), `launch-{role}.sh` (généré) | `runtime_mode.rs:85` (resolver canonique), `vector_control.rs:1476`, `embedder.rs:*` |
| `AXON_EMBEDDING_PROVIDER_EFFECTIVE` | Résultat après init | `embedder.rs:3105-3122`, `provider_runtime.rs:42` | `embedder.rs:1946`, `provider_runtime.rs:31`, `status.sh:132` |
| `AXON_EMBEDDING_PROVIDER_INIT_ERROR` | Raison du fallback | `provider_runtime.rs:44-45` | `provider_runtime.rs:103`, heartbeat |
| `AXON_EMBEDDING_GPU_PRESENT` | Détection GPU au boot | `runtime_boot.rs:520` | `embedder.rs:1167` |
| `AXON_GPU_ACCESS_POLICY` | Vestige policy "avoid/shared/preferred" | `start.sh:543`, `start-brain.sh:8`, `start-indexer.sh:8`, `axon-resource-policy.sh:213-217` | `resource-policy.sh` (défauts) |
| `AXON_GPU_EMBED_SERVICE_ENABLED` | Toggle subprocess GPU service | `start.sh:510` (sous --tensorrt) | `ort-runtime.sh`, `embedder.rs` |
| `AXON_GPU_EMBED_SERVICE_TENSORRT` | Toggle TensorRT EP | `start.sh:511` | `ort-runtime.sh:24`, `embedder.rs` |
| `AXON_REQUEST_TENSORRT` | Override implicit --tensorrt | `start-indexer.sh:24` | `start-indexer.sh:24` |
| `AXON_GPU_TELEMETRY_BACKEND` | NVML vs autre | `start.sh:513` | embedder code |
| `AXON_NVML_LIBRARY_PATH` | Chemin lib NVML | `start.sh:520-528` | embedder code |
| `AXON_POLICY_SOURCE_AXON_EMBEDDING_PROVIDER` | Méta-trace "explicit/policy_default" | `axon-resource-policy.sh:181,249` | nulle part de critique |

### B. Flags CLI (3 alias pour la même chose)
- `--tensorrt` → flip `AXON_EMBEDDING_PROVIDER=cuda` + `AXON_GPU_EMBED_SERVICE_TENSORRT=1` (start.sh:472, 502-509)
- `--gpu-backend tensorrt` (bench scripts) → idem
- (Pas de `--cpu` ni `--gpu` explicites)

### C. Logiques de défaut (4 sources de "défaut")
1. **`canonical_embedding_provider_request_for_mode`** (`runtime_mode.rs:77`) — vérité actuelle :
   ```
   if !mode.semantic_workers_enabled(): "cpu"     ← BrainOnly/IndexerGraph → cpu
   elif env(AXON_EMBEDDING_PROVIDER) set: use it
   elif gpu_present: "cuda"
   else: "cpu"
   ```
2. **`start.sh:589-598`** — bash resolver, **redondant** avec #1 :
   ```bash
   if mode not in {full,vector}: cpu
   elif env(AXON_EMBEDDING_PROVIDER) set: use it
   elif detect_accessible_gpu: cuda
   else: cpu
   ```
3. **`start-brain.sh:8`** : `AXON_GPU_ACCESS_POLICY=avoid` par défaut
4. **`start-indexer.sh:8`** : `AXON_GPU_ACCESS_POLICY=shared` par défaut + `AXON_REQUEST_TENSORRT=1` implicite si mode full/vector

### D. Mécanismes de coercion (2 actifs)
1. **`canonical_embedding_provider_request_for_mode`** force `cpu` pour modes brain/graph (côté Rust, OK — c'est physiquement correct)
2. **`start.sh:589`** force `cpu` pour modes non-{full,vector} (côté bash, REDONDANT avec #1)
3. ~~`axon-resource-policy.sh:247-249`~~ (supprimée par REQ-AXO-184 #1 — commentaire confirmé en place)

### E. Fichiers d'état persistants
- `.axon/run-indexer/runtime.env` — pour heartbeat reload (4 vars seulement, pas `AXON_EMBEDDING_PROVIDER` actuellement)
- `.axon/run-{brain,indexer}/launch-{role}.sh` — généré à chaque start, contient `export AXON_EMBEDDING_PROVIDER=...` figé

### F. Sites Rust de mutation directe (env::set_var) — anti-pattern
- `runtime_boot.rs:518, 933, 946, 959, 968, 977, 990, 998, 1015` (9 sites — boot + tests)
- `embedder.rs:3105, 3109, 3113, 3118, 3122, 3199, 3237, 3548, 3561, 3584, 3592, 3604, 3612, 3643, 3650, 3775, 3782, 4234` (18 sites — init success/failure + 14 tests)
- `provider_runtime.rs:42, 44-45` (2 sites — register hooks)

**Total : 29 sites Rust qui muent env vars de l'embedder.** Anti-pattern lourd.

## Architecture cible

### Une seule structure (Rust)
```rust
// src/axon-core/src/embedder/provider_config.rs (NEW)

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EmbedderProvider {
    Cpu,
    Cuda,
    Tensorrt,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EmbedderProviderState {
    pub requested: EmbedderProvider,
    pub effective: Option<EmbedderProvider>,  // None until init completes
    pub init_error: Option<String>,
    pub gpu_present: bool,
    pub runtime_mode: AxonRuntimeMode,
}

static EMBEDDER_PROVIDER_STATE: OnceLock<RwLock<EmbedderProviderState>> = OnceLock::new();

impl EmbedderProviderState {
    /// Single entry point : called exactly once at boot.
    /// Reads AXON_EMBEDDING_PROVIDER from env, resolves with mode + GPU detection,
    /// stores result. After this, NO env::set_var anywhere.
    pub fn boot(runtime_mode: AxonRuntimeMode) -> Self { ... }

    /// Called by embedder.rs after EP init attempt.
    pub fn record_effective(provider: EmbedderProvider, init_error: Option<String>) { ... }

    /// Read-only accessor — used by heartbeat, status, vector_control, etc.
    pub fn current() -> EmbedderProviderState { ... }
}
```

### Une seule variable env (bash → rust handoff)
- `AXON_EMBEDDING_PROVIDER` = la **seule** input opérateur (valeurs : `cpu`, `cuda`, `tensorrt`, ou unset = auto)
- Tout le reste (`_EFFECTIVE`, `_INIT_ERROR`, `_GPU_PRESENT`, `AXON_GPU_ACCESS_POLICY`, `AXON_GPU_EMBED_SERVICE_*`, `AXON_REQUEST_TENSORRT`) **supprimé**.

### Un seul flag CLI
- `--embedder cpu|cuda|tensorrt|auto` (alias dépréciés : `--tensorrt` → `--embedder tensorrt` avec warning de dépréciation)
- `--tensorrt` toléré 1 release puis retiré.

### Une seule logique de défaut
- Côté Rust uniquement (`EmbedderProviderState::boot()`).
- Bash ne décide PLUS : il propage juste `AXON_EMBEDDING_PROVIDER` si l'opérateur l'a explicitement settée (ou rien).
- Disparition des 4 logiques redondantes.

### Une seule source pour le heartbeat / status / monitoring
```rust
heartbeat["embedder_provider"] = EmbedderProviderState::current().to_json();
// → { requested: "cuda", effective: "cuda", init_error: null, gpu_present: true, runtime_mode: "indexer_full" }
```

## Plan de migration ATOMIQUE (1 PR, pas incrémental)

### Phase A — Code Rust (1 PR)
1. **Créer** `src/axon-core/src/embedder/provider_config.rs` avec la struct + boot/record/current
2. **Supprimer** `src/axon-core/src/embedder/provider_runtime.rs` (29 sites env::set_var)
3. **Supprimer** dans `runtime_boot.rs:518-525` les `env::set_var` ; appeler `EmbedderProviderState::boot()` à la place
4. **Adapter** `embedder.rs:1167, 1946, 3105-4237` : remplacer toutes les lectures/écritures env par `EmbedderProviderState::current()` / `::record_effective()`
5. **Adapter** `vector_control.rs:1476` : `EmbedderProviderState::current().effective`
6. **Adapter** `runtime_mode.rs:85` : exposer `canonical_embedding_provider_request_for_mode` qui lit env UNE SEULE FOIS au boot, expose un getter pur
7. **Adapter** `main_telemetry.rs:126-161` : `EmbedderProviderState::current().to_heartbeat_json()`

### Phase B — Scripts bash (1 PR, même commit)
1. **Supprimer** `start.sh:589-598` (resolver bash redondant)
2. **Supprimer** `start.sh:510-543` (tensorrt block — collapse dans une seule ligne `export AXON_EMBEDDING_PROVIDER=tensorrt`)
3. **Supprimer** `start-brain.sh:8` et `start-indexer.sh:8` (les `AXON_GPU_ACCESS_POLICY` defaults — variable supprimée)
4. **Supprimer** `start-indexer.sh:14-26` (l'`AXON_REQUEST_TENSORRT` + tensorrt_flag implicite). Remplacer par : si `AXON_EMBEDDING_PROVIDER` unset et mode={full,vector}, propager rien → Rust décide.
5. **Supprimer** `axon-resource-policy.sh:181,186` (la liste qui inclut `AXON_EMBEDDING_PROVIDER` dans le scoped_var loop)
6. **Adapter** `status.sh:132` : afficher `EMBEDDER` (provider effectif via API ou heartbeat read) au lieu de `EMBED  $AXON_EMBEDDING_PROVIDER`
7. **Adapter** `start.sh:1018-1050` (génération launch-{role}.sh) : retirer `${EMBEDDING_PROVIDER_EXPORT}` du template. Propager via env naturel uniquement.

### Phase C — Tests
1. **Supprimer** `src/axon-core/src/tests/embedder_provider_runtime_tests.rs` (correspond à l'ancien provider_runtime.rs)
2. **Créer** `provider_config_tests.rs` :
   - boot(mode=BrainOnly, gpu=true) → Cpu
   - boot(mode=IndexerGraph, gpu=true) → Cpu
   - boot(mode=IndexerFull, gpu=true, env unset) → Cuda
   - boot(mode=IndexerFull, gpu=false, env unset) → Cpu
   - boot(mode=IndexerFull, gpu=true, env="cpu") → Cpu (override respecté)
   - boot(mode=IndexerFull, gpu=true, env="tensorrt") → Tensorrt
   - record_effective(Cpu, Some("init failed")) → state shows fallback
3. **Adapter** `scripts/test_axon_resource_policy.sh` : retirer les assertions sur `AXON_EMBEDDING_PROVIDER` (n'est plus settée par policy)

### Phase D — Documentation
1. **Mettre à jour** `docs/operations/2026-04-18-live-dev-runtime-operations.md`
2. **Mettre à jour** `docs/audits/2026-05-22-env-vars-inventory.md` : enlever 10 vars du tableau
3. **Mettre à jour** `docs/vision/SOLL_EXPORT_*` : noter la résolution
4. **SOLL** : créer ce REQ-AXO-901737 + linker à REQ-AXO-184 (REFINES) + DEC-AXO-070 (REFINES)

## Acceptance criteria
- [ ] `grep -rn "AXON_EMBEDDING_PROVIDER_EFFECTIVE\|AXON_EMBEDDING_PROVIDER_INIT_ERROR\|AXON_EMBEDDING_GPU_PRESENT\|AXON_GPU_ACCESS_POLICY\|AXON_GPU_EMBED_SERVICE\|AXON_REQUEST_TENSORRT" src/ scripts/ → 0 résultats** hors tests legacy supprimés
- [ ] `grep -rn "env::set_var.*EMBEDDING_PROVIDER" src/ → 1 résultat unique` (le boot)
- [ ] `scripts/start.sh` ne contient plus de logique `if RUNTIME_MODE == ...` autour de l'embedder
- [ ] `launch-{role}.sh` généré ne contient PLUS `export AXON_EMBEDDING_PROVIDER=`
- [ ] `axon status` affiche `EMBEDDER cuda` (provider effectif live, pas via env)
- [ ] Heartbeat JSON `embedder_provider.requested` et `effective` cohérents avec runtime réel
- [ ] Lancer `AXON_EMBEDDING_PROVIDER=cuda ./scripts/axon-live start --indexer-full` → heartbeat confirme `effective=cuda` (sinon `init_error` rempli avec raison claire)
- [ ] Lancer sans var d'env + GPU présent + mode full → auto-détection cuda
- [ ] Smoke test : restart 3 fois consécutifs, heartbeat cohérent à chaque démarrage
- [ ] Tests cargo passent : `cargo test --manifest-path src/axon-core/Cargo.toml`

## Effort estimé
- Phase A (Rust) : ~6h (29 sites à refactorer + tests)
- Phase B (bash) : ~2h
- Phase C (tests) : ~2h
- Phase D (docs/SOLL) : ~1h
- **Total : ~1.5 jour dev + 0.5 jour QA = 2 jours**

## Risques
- **Régression sur le mode auto-détection** : si `gpu_present` mal détecté côté Rust (déjà un risque actuel)
- **Tests legacy** : ~14 tests dans `embedder.rs:3199-4237` manipulent env::set_var pour mocker des scénarios. Migration vers struct-based testing nécessaire.
- **`AXON_GPU_ACCESS_POLICY`** est utilisé hors embedder pour d'autres décisions (ex : worker count, watcher policy). Suppression à valider — peut-être renommer en `AXON_GPU_TELEMETRY_ENABLED` ou similaire.

## Pourquoi atomique (pas incrémental)
Le problème vient justement de la fragmentation par migrations incrémentales successives (REQ-AXO-184 a déjà commencé). Une nouvelle migration partielle ajoute une 4ème logique de défaut à côté des 3 autres. La règle Swiss-hiking (GUI-AXO-1023) : la faiblesse est résolue ou annoncée — pas moitié-moitié. Cette PR migre **tout en bloc** ou annule.

## Originator
LLM, suite à diagnostic forensique 2026-05-23 (SSD saturé par 2 indexers en CPU silent-fallback) + directive opérateur :
"C'est une grave erreur cette idée de CPU au lieu de GPU. On a déjà eu le problème plusieurs fois, il faut simplifier ces informations."
