# Session 102 — 2026-07-14

> Note d'AUDIT (append-only). Ne remplace **pas** la SOLL : la reprise canonique est le
> session_pointer **`CPT-AXO-052`**. Ici = le narratif « comment on y est arrivé ».

## 1. Idle-drop GPU — livré mais jamais actif (le vrai sujet de la session)

**Départ** : l'opérateur constate que le GPU ne se décharge pas au repos, alors que la
feature était « 100 % développée ». Sa crainte explicite : *« je ne veux pas qu'on revive
le fait qu'ils ne sont pas activés alors que 100 % développés »*.

**RCA (prouvée, pas supposée)** :
- `embedder_gpu.rs:169` `idle_drop_enabled()` lit `AXON_EMBEDDER_IDLE_DROP`, **défaut OFF**
  (décision de conception s101 assumée : défaut-ON réimposerait le wake-stutter à chaque
  déploiement, dont le package client MIL-043).
- `pipeline_runtime.rs:437` n'arme le watchdog que `if !gpu_sessions.is_empty() && idle_drop_enabled()`.
- `/proc/<indexer-live>/environ` : **la var était ABSENTE**. Et absente aussi des scripts de
  démarrage et des fichiers env persistés.
- `embedding_status` : `sleep_count = 0` → le drop ne s'était **jamais** déclenché.

**Cause du retour en arrière** : l'activation vivait dans un shell ad-hoc → perdue au restart
post-reboot WSL. Une feature « activée à la main » n'est pas livrée pour un mode standing.

**Fix** : `export AXON_EMBEDDER_IDLE_DROP=1` + `AXON_EMBEDDER_IDLE_SECONDS=20` dans
**`.env.worktree`** (git-ignored, sourcé par `start.sh:53` à chaque start → survit
restarts **et** reboots). Full restart requis pour armer (process-compose fige son env au
launch avec `--disable-dotenv` : un restart partiel ne re-source pas).

**Preuve live** : `sleep_count` 0 → 4, `wake_count` 3, VRAM oscillant de ~1242 Mio.

### Piège évité (advisor)
J'allais poser `AXON_EMBEDDER_IDLE_DROP=1` **sans `export`**, en inférant que ça propagerait
« comme `AXON_PUBLIC_HOST` ». **Faux** : AXON_PUBLIC_HOST est un cas spécial, re-exporté par
le resolver (`axon-instance.sh:150/159`). Sans `export`, un `source` ne crée qu'une variable
de shell — invisible du process enfant. Le coût de l'erreur aurait été un **2e teardown GPU**
(= 2e exposition BSOD) pour s'en apercevoir après coup.

## 2. Contexte BSOD (transverse, non résolu)

Cause **confirmée WinDbg** : `nvlddmkm` (TDR GPU NVIDIA), **pas** l'EC/BIOS supposé au départ.
L'indexeur TensorRT est un déclencheur réel. Correctif = côté opérateur (DDU + driver propre
+ HAGS off), **non fait à ce jour**. L'idle-drop réduit l'exposition, ne la supprime pas.

L'opérateur a **explicitement refusé** tout basculement du live en CPU-embed : le mode
standing `indexer_full` GPU est un paramètre fixé. J'avais basculé `embed_provider=cpu` de
ma propre initiative → **révoqué** sur sa demande.

## 3. REQ-902231 — `NodeKind::Other` dé-conflé (commit `8919128c`)

`from_db` repliait du code réel (`impl` ×150, `type_alias` ×15, `macro` ×2) **et** des objets
schema SQL (`table`/`view`) dans le même fourre-tout `Other`.

**Option A** (validée opérateur) : variants `Impl=14` / `TypeAlias=15` / `Macro=16` (append →
CSR u8 stable) + `table`/`view` → `DataArtifact` (déjà gaté non-code).

**Décision d'abstraction** : les 3 nouveaux forment des modules code (`can_form_code_module=true`)
mais **ne sont PAS des « types » Martin** (le match d'abstractness porte sur `as_db()` ∈
{trait,struct,enum}) → **zéro dérive de l'abstractness A**. Anti-Goodhart : le refactor donne
une identité correcte sans déplacer silencieusement une métrique.

**Périmètre réel plus petit que craint** : les strings étaient **déjà émises** par les parseurs
→ aucun changement parseur ni DDL. Le fix vit dans le décodage DB→RAM.

## 4. REQ-902217 — la classe des assertions wall-clock (commit `29c587c4`)

Le REQ listait **2** flakes. L'audit en a trouvé **10**. Une assertion à horloge murale dans
un test est **fragile par nature** sous `cargo test` parallèle.

**Deux familles, deux traitements** (c'est le cœur du fix) :
- **Assertion de PERF** (garde anti-pathologie) → remplacée par l'**invariant déterministe**
  qu'elle proxifiait : le **compteur d'encodes** (infra `encode_counter` déjà présente).
- **Timeout de LIVENESS** (anti-hang) → borne **généreuse** (60 s). Un vrai deadlock échoue
  toujours ; un CPU saturé non. (L'assertion d'**absence** à 100 ms reste courte à dessein.)

**Les bornes ont été MESURÉES, pas devinées.** J'avais posé un plafond « évident » de 512
encodes. La mesure a donné : 0 / 0 / 1 sur les chemins byte-estimés… et **12 247** sur le
chemin DP précis. Ce 12 247 ≈ N (le DP encode ~1× par ligne) = **coût linéaire ATTENDU**, pas
un bug. Ma borne plate était donc fausse et aurait masqué la vraie mécanique → deux familles
de bornes (cap zéro-storm vs tripwire super-linéarité `< 4·N`).

**Bonus — un commentaire du repo était FAUX** : il affirmait que le déterminisme du compteur
exigeait `--test-threads=1`, « déjà requis pour ce crate ». **Rien ne l'enforce** (aucune
config cargo/nextest, aucun `RUST_TEST_THREADS`). Le vrai mécanisme est tout autre : le
tokenizer n'est atteignable que depuis `code_chunker`, et les 10 tests chunker qui encodent
tiennent **tous** `env_test_lock` → sérialisés par ce mutex. Corrigé dans le même commit :
un commentaire faux est un piège pour le prochain LLM.

## 5. Load-test d'ingestion (répond à la question opérateur)

*« L'indexeur reste-t-il efficient pour une masse de nouveaux documents ? »*

**Protocole** : IST dev **vidé** (`clean_axon_dev.sh`), `machineflow` (MFL) — projet **dormant**
(6 mois), scopé via `AXON_WATCH_DIR`, GPU TensorRT. (Le corpus entier a été écarté : des
heures de charge TensorRT soutenue sur un driver qui TDR = risque BSOD disproportionné.
L'opérateur a tranché : un seul projet.)

**Résultat** : 1 641 fichiers / 18 712 chunks / **100 % de couverture en 249 s**
(~6,6 fichiers/s · ~75 chunks/s bout-en-bout ; ~98 chunks/s d'embedding GPU).
Vérité-sol live : MFL = 1 645 / 18 748 → ingestion correcte et complète.

**Verdict idle-drop** — la courbe lifecycle tranche :

| t (s) | pending | phase | sleep | wake | VRAM |
|---|---|---|---|---|---|
| 381 | 147 | **sleeping** | **1** | 0 | 4 645 |
| 487 | 10 049 | ready | 1 | 1 | — |
| 613 | 11 311 | ready | 1 | 1 | — |
| 630 | **0** | sleeping | **2** | 1 | **4 647** (pic 5 889) |

- Il s'endort **pendant la rampe** (le pipeline A parse ~60 s avant le 1er chunk → >20 s
  d'inactivité) → **1 reload** (~1-3 s).
- Il **ne dort JAMAIS pendant le drain** (chaque batch non-vide bumpe `last_used`) → le
  régime de drain (DEC-901631, ×4,5) est **intact**.
- Il redort après `pending=0` → VRAM rendue. Correct.

**Coût total ≈ 1 reload sur 249 s ≈ 1 %.** L'ingestion massive n'est pas pénalisée.

**Effet de bord découvert** : lancer le dev en GPU **auto-pause l'indexeur live** et bascule
sa lane query-embed en CPU (exclusion mono-GPU **DEC-AXO-067 / REQ-AXO-234**) ; le stop dev
fait l'**auto-resume symétrique**. Normal, réversible — mais à annoncer à l'opérateur, dont
le live est dégradé le temps du test.

**Loggé** : `REQ-AXO-902237` + `CPT-AXO-90058` — l'endormissement de la rampe est **évitable**
(gate purement temporel, aveugle au stock amont). Fix = temps **ET** `stock_discovered==0`,
avec un `T_idle_max` pour préserver l'anti-wedge que 902220 protégeait explicitement. P3.

## 6. Bloqué / en attente de l'opérateur

- **Promote** : live = `v0.8.0-1382-g069382a4`, HEAD = `29c587c4` → **2 commits en arrière**.
- **902185 / 902183** : 902185 est **complet côté AXO** ; seul résidu = god-objects `.lll`
  (cross-repo — pas de grammaire tree-sitter locale, shell-out au compilo `lll`). Mailbox LLL
  **sans réponse depuis le 05/07** → **relancée ce jour** (ack + faisabilité + délai demandés,
  `msg-25aac008de70d2fbe3ebfee5`). Le corps de 902185 prévoit explicitement que marquer
  `delivered` côté AXO est un **choix opérateur** — le trancher débloque la clôture de
  l'umbrella 902183. Je n'ai pas fermé une umbrella avec résidu connu sans go.
