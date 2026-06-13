# Session 76 — 2026-06-13 — pipeline-muet fix + embed-throughput diagnosis

Audit-only narrative. Canonical actionable state = session_pointer `CPT-AXO-052`.
Embed-throughput diagnosis = `CPT-AXO-90047`.

## Livré (committed, main, HEAD d9fb007c, 23 commits non poussés)

1. **REQ-AXO-901970** — RAM-exclusif pour toutes les analytics structurelles + audit-score
   (security/tech-debt/telemetry/circular). Dernière SQL graphe `ist.Edge` supprimée des
   analytics. Ajout colonne `name` canonique au snapshot RAM (IstGraph) car les analytics
   par-nom (TODO/secret/macro) avaient besoin du vrai `ist.symbol.name`, découplé du suffixe
   d'id. Commits e65a2de2, 086608ec (+ antérieurs).
2. **REQ-AXO-901949** — tracer-bullet optimal-pour-LLM (schemars/repair-en-donnée/terse) :
   vérifié + delivered.
3. **REQ-AXO-901959** — bulk_writer COPY sur le pool natif du GraphStore + preuve de routage
   explicite (a66e997e) ; validé sous charge réelle (Plane A ~900/s, COPY actif).
4. **REQ-AXO-140** — retrait du lookup PG petit-batch ≤16 redondant (la résolution cross-file
   CALLS se fait dans le snapshot RAM), commit 85311cc3.

## Le gros morceau : « pipeline muet » (marchait il y a 1-2 jours)

Symptôme : pipeline dev n'indexe rien (Plane A = 0). RCA :
- **Cause racine** : `fs.inotify.max_user_instances` revenu à 128 (défaut kernel) après un
  restart WSL — WSL2 sans `systemd` n'applique pas `/etc/sysctl.d/99-axon-inotify.conf` (=1024).
  ET le guard de lancement `axon-os-limits.sh` faisait `sysctl -w` SANS sudo → échec silencieux
  → restait à 128. À 128, Watchman sature sur les ~15 roots de `/home/dstadel/projects`, tous
  les watch-project échouent → fallback scanner → 0 fichier.
- **Bug latent réveillé** : le fallback scanner paniquait — le matcher `.git/info/exclude` était
  rooté à `.git/info` (parent du fichier) au lieu du repo dir → `ignore::Gitignore` panic
  « path is expected to be under the root » sur chaque repo avec un exclude file → tâche tokio
  tuée → fichiers droppés.

Fixes : `faae1173` (guard inotify via sudo -n, self-heal à chaque launch), `d9fb007c` (matcher
rooté au repo dir + test régression), `61e88e76` (clean_axon_dev.sh ist.* au lieu de public.*).
Re-validé sur run propre : Plane A ~1200/s, index complet 14342 fichiers (= total éligible,
reconciliation walk fed 18890/18890), 0 panic.

## Ouvert : embed-throughput (CPT-AXO-90047) — session perf DÉDIÉE

Plane B draine à ~35 ch/s, GPU à 1% (affamée). Trois facteurs liés :
1. Optimiseur (optimizer.rs:551/1110) dimensionne les batches sur le backlog IN-PROCESS
   (front A→B + outbox), pas le backlog PG réel → en drain-only rétrécit chunk_batch_size +
   micro_batch à 4-16.
2. Token-bucketing (build_token_aware_micro_batches) éclate les chunks de longueurs diverses
   en buckets de 4-16.
3. **TensorRT shape FIXE (décisif)** : moteur IoBinding batch=64 ; `embedder-bench --sweep-batches`
   ÉCHOUE dès batch=8 (`setInputShape` hors profil). Lead central pour la session dédiée.

Fix spéculatif (micro_batch → cible) tenté, INEFFICACE (~23 ch/s), reverté. Mesures live trop
bruitées (système chargé, nvidia-smi hang). Conclusion disciplinée : NE PAS patcher en live ;
réparer le bench contrôlé + activer les timings breakdown (tokenize_ms/inference_ms, jetés dans
embedder_gpu.rs) AVANT tout changement.

## Méthode retenue (leçon)
Mesurer perf sur bench CONTRÔLÉ dès le départ, pas sur le drain live d'un système chargé.
Cf. mémoire feedback_load_test_clean_ist_not_bench + feedback_toc_discipline_for_pipeline_debug.
