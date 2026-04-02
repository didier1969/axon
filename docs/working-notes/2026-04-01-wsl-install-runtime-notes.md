# WSL Install and Runtime Notes

Date: 2026-04-01
Context: mise en service Axon sur WSL pour usage réel projet
Status: in-progress

## Objective

Tracer les petits défauts, frictions et interventions pendant la phase d'installation/démarrage WSL, afin de corriger ensuite tout ce qui n'est pas automatique.

## Observed Facts

### 1. Shell courant non valide hors Devenv

Constat:

- `bash scripts/validate-devenv.sh` échoue immédiatement hors `devenv shell`
- les variables canoniques (`HEX_HOME`, `CARGO_TARGET_DIR`, `RELEASE_COOKIE`, `PHX_PORT`, `HYDRA_HTTP_PORT`, `LIBCLANG_PATH`, `PYTHONPATH`) manquent alors
- les toolchains visibles hors shell canonique proviennent de `mise` / `~/.cargo` et non de Nix/Devenv

Impact:

- on ne peut pas diagnostiquer ni lancer Axon proprement depuis un shell WSL arbitraire

Intervention:

- bascule explicite sur le workflow officiel `devenv shell -- bash -lc '...'`

Correctness:

- comportement attendu, mais friction d'onboarding réelle à documenter plus visiblement

### 2. Bootstrap officiel fonctionne sans intervention manuelle

Constat:

- `devenv shell -- bash -lc './scripts/setup_v2.sh'` a compilé Rust, compilé Elixir et exécuté les tests sans intervention opérateur

Impact:

- bon point: l'installation officielle est automatisée sur WSL quand le chemin canonique est respecté

Intervention:

- aucune correction manuelle nécessaire

### 3. Démarrage officiel fonctionne sans intervention manuelle

Constat:

- `devenv shell -- bash -lc './scripts/start-v2.sh'` a démarré le runtime, le dashboard, la surface SQL et la surface MCP
- la vérification MCP intégrée du script est passée

Impact:

- bon point: le chemin de démarrage officiel est opérable tel quel sur WSL

Intervention:

- aucune correction manuelle nécessaire

### 4. Signal de fin d'indexation ambigu

Constat:

- les types `ScanStarted` / `ScanComplete` existent côté bridge Rust
- mais le chemin runtime principal observé n'émet pas clairement `ScanComplete` comme signal exploitable de fin de scan initial

Impact:

- on ne peut pas, de façon propre et déterministe, attendre la fin d'indexation initiale sur ce seul événement
- pour l'exploitation, il faut actuellement inférer la stabilisation via d'autres signaux runtime

Intervention:

- surveillance prévue via stabilisation de la profondeur de queue et quiescence observable du runtime

Corrective follow-up:

- rendre explicite et fiable un événement canonique `initial_scan_complete` ou équivalent sur le bridge Rust

### 5. Formulation documentaire potentiellement trompeuse

Constat:

- l'expression "démarrage quotidien" peut être comprise comme "scan une fois par jour"

Impact:

- ambiguïté pour un opérateur humain

Intervention:

- clarification immédiate: Axon fait un scan initial au boot puis reste en surveillance continue avec ingestion des deltas

Corrective follow-up:

- vérifier si README / getting-started doivent expliciter encore mieux `scan initial + watch continu`

### 6. Observation différée via `tmux` non fiable depuis ce shell

Constat:

- une capture immédiate du pane `tmux` Axon fonctionne
- une capture différée via un shell temporisé a échoué avec `error connecting to /tmp/tmux-1000/default (Operation not permitted)`

Impact:

- l’observation opératoire ne doit pas dépendre uniquement de `tmux` quand on automatise la validation depuis ce contexte

Intervention:

- repli sur les surfaces canoniques SQL/MCP pour suivre la stabilisation du runtime

Corrective follow-up:

- si l’on veut une observabilité totalement scriptable, exposer un état de fin de scan et de queue idle directement sur MCP ou SQL, sans dépendre d’un scrape `tmux`

### 7. Intermittence de la surface HTTP `44129` pendant le scan initial

Constat:

- juste après le démarrage, `start-v2.sh` valide bien SQL et MCP sur `44129`
- pendant la phase de scan/réscan initiale, certains sondages `curl` et `nc` vers `127.0.0.1:44129` échouent transitoirement
- en parallèle, le runtime continue manifestement à tourner et à scanner dans `tmux`

Impact:

- la disponibilité opératoire de SQL/MCP pendant l’indexation initiale n’est pas suffisamment stable pour un client externe si ce comportement se confirme

Intervention:

- repli d’observation sur:
  - `tmux` pour l’activité scanner
  - SQL/MCP quand la surface redevient joignable

Corrective follow-up:

- diagnostiquer pourquoi `44129` devient intermittente pendant le scan initial lourd alors que le process `axon-core` reste vivant
- vérifier si le serveur HTTP est bloqué, affamé CPU, ou si le symptôme vient du contexte WSL/shell

### 8. Les sondages différés héritent de restrictions du shell courant

Constat:

- certaines vérifications immédiates (`tmux capture-pane`, `ss`) fonctionnent
- les mêmes vérifications exécutées après temporisation dans un sous-shell échouent avec `Operation not permitted`

Impact:

- l’automatisation de l’observation depuis ce contexte doit éviter les sous-shells temporisés pour ces outils

Intervention:

- retour à des sondages immédiats et séquentiels

Corrective follow-up:

- si l’on veut des checks différés fiables, fournir un script de diagnostic runtime dédié plutôt que dépendre de `tmux` / `ss` directement

### 9. Le dashboard actuel ne répond pas au besoin produit

Constat utilisateur explicite:

- le dashboard actuel n’est pas considéré comme utilisable
- il n’expose essentiellement que quelques paramètres machine / runtime
- il ne fournit pas la profondeur fonctionnelle attendue d’un vrai cockpit Axon

Impact:

- même si le runtime et le MCP tournent, la surface Phoenix actuelle n’est pas un livrable UX satisfaisant pour piloter, comprendre et exploiter Axon

Intervention:

- exigence enregistrée: le dashboard doit être repensé entièrement, pas seulement poli

Corrective follow-up:

- ouvrir un chantier dédié de refonte complète du dashboard
- repartir des besoins réels:
  - état d’indexation utile
  - santé de l’ingestion
  - vérité projet / workspace
  - couches dérivées réellement exploitables
  - valeur développeur, pas seulement télémétrie machine

### 10. Incohérence entre activité runtime réelle et statuts d’indexation exposés

Constat:

- les logs runtime montrent un scan/staging actif en continu (`watcher.db_upsert`, `watcher.staged`)
- au même moment, un snapshot SQL a retourné:
  - `pending = 34330`
  - `indexed = 14473`
  - `skipped = 8`
  - `indexing = 0`
- cela ne correspond pas à l’image attendue d’un pipeline vivant où une partie du backlog devrait transiter par l’état `indexing`

Impact:

- l’observabilité d’ingestion n’est pas fiable telle quelle
- un opérateur peut conclure à tort que rien n’est en cours de traitement alors que le runtime travaille encore
- ce défaut est potentiellement plus grave qu’un simple problème d’affichage, car il peut révéler une incohérence entre le runtime réel et la machine d’état persistée

Intervention:

- le diagnostic opératoire courant ne doit pas s’appuyer uniquement sur le champ `status`
- recouper systématiquement:
  - activité runtime observable
  - snapshot SQL
  - réponse MCP quand disponible

Corrective follow-up:

- vérifier si l’état `indexing` est encore correctement écrit par le chemin canonique d’ingestion Rust
- vérifier si le scanner remplit `pending` plus vite que le claimer ne marque `indexing`
- vérifier si la sémantique du champ `status` a divergé du comportement runtime réel

### 11. `axon_debug` emploie actuellement un libellé trompeur pour les fichiers

Constat:

- `axon_debug` a reporté `Fichiers indexés : 48811`
- dans le même intervalle, le SQL direct a montré:
  - `count(File) = 48811`
  - `status = indexed` seulement `14473`
- le compteur affiché par `axon_debug` correspond donc en pratique au total des fichiers connus, pas au total des fichiers réellement terminés

Impact:

- l’outil de diagnostic principal surestime fortement la complétion
- un utilisateur ou un agent peut croire que presque tout est indexé alors qu’une grande partie du backlog reste en attente

Intervention:

- relecture manuelle du snapshot opératoire avec distinction explicite:
  - `fichiers connus`
  - `fichiers terminés`

Corrective follow-up:

- corriger `axon_debug` pour fusionner:
  - volume graphe
  - backlog d’indexation
  - taille de base
- renommer le compteur actuel pour qu’il reflète la réalité (`fichiers connus` au lieu de `fichiers indexés`)

### 12. Les timings et coûts d’indexation ne sont pas historisés dans la base

Constat:

- la table `File` ne contient pas de colonnes `started_at`, `indexed_at`, `duration_ms`, `parse_ms`, `commit_ms`, `rss_bytes` ou `peak_memory_bytes`
- le runtime calcule bien des timestamps internes `t0..t4` pour un fichier et une estimation de coût observé basée sur taille + durée
- mais ces informations sont émises sur le flux runtime / tracer, pas persistées comme historique exploitable dans DuckDB

Impact:

- on ne peut pas répondre proprement, a posteriori, à:
  - heure exacte d’indexation d’un fichier
  - durée totale d’indexation complète d’un scan
  - distribution des durées par fichier / parser / taille
  - ratio taille fichier / temps d’indexation / coût mémoire réellement observé

Intervention:

- diagnostic actuel limité à:
  - statuts SQL courants
  - télémétrie runtime courante
  - métriques éphémères en mémoire ETS côté dashboard

Corrective follow-up:

- ajouter une historisation canonique des événements d’indexation, au minimum:
  - `queued_at`
  - `started_at`
  - `finished_at`
  - `duration_ms`
  - `processing_mode`
  - `status_final`
  - `size_bytes`
  - `parser_key`
  - `estimated_cost_bytes`
  - `observed_cost_bytes`
- décider explicitement si l’on veut aussi persister:
  - RSS runtime au moment du traitement
  - proxy de mémoire par fichier
  - percentiles par parser / bucket de taille

### 13. Accélération matérielle partiellement exploitée seulement

Constat:

- le runtime détecte la présence d’un GPU dans `runtime_profile`
- mais le chemin courant des embeddings initialise `fastembed::TextEmbedding` sans configuration explicite de provider GPU
- Axon utilise déjà des bibliothèques qui peuvent profiter du CPU moderne (`rayon`, ONNX Runtime via `fastembed`, DuckDB), mais il n’existe pas aujourd’hui de stratégie explicite documentée pour:
  - activer un provider GPU ONNX
  - profiler les gains par phase
  - brancher des profils SIMD/CPU spécifiques selon la machine

Impact:

- les gains CPU/SIMD sont probablement déjà partiellement implicites via les dépendances, mais non pilotés finement
- le GPU disponible n’est pas garanti d’apporter un gain aujourd’hui, faute d’activation explicite du chemin GPU
- on ne peut pas estimer proprement le ROI d’une optimisation matérielle sans benchmark par phase

Corrective follow-up:

- instrumenter les durées par phase (scan, parse, write DB, embeddings)
- vérifier le support réel de providers GPU dans la version `fastembed` / `ort` retenue par Axon
- décider si l’on introduit:
  - un profil CPU optimisé
  - un profil GPU embeddings
  - un fallback strict CPU pour WSL sans pile CUDA valide

### 14. Les lectures MCP et SQL ne sont pas encore isolées du contexte writer

Constat:

- le serveur HTTP MCP/SQL est bien dispatché rapidement par Axum via `spawn_blocking`
- le pool DuckDB crée bien deux contextes, `writer_ctx` et `reader_ctx`
- en pratique, les requêtes de lecture haut niveau (`query_json`, `query_json_param`, `query_count`, `execute`) passent encore par `writer_ctx`
- les opérations d’ingestion et de claim passent elles aussi par `writer_ctx`
- la priorité des lectures MCP n’est donc pas garantie par une vraie voie de lecture séparée: lectures et écritures se disputent le même mutex writer

Impact:

- une requête MCP peut être retardée par un claim ou un lot d’ingestion en cours
- la logique actuelle de protection passe surtout par `service_guard`, qui observe les latences et ralentit indirectement les claims si MCP/SQL deviennent lents
- cela explique probablement l’impression d’intermittence MCP sous forte indexation: le dispatch HTTP est rapide, mais pas l’accès effectif à la base

Corrective follow-up:

- faire transiter les lectures MCP et SQL read-only sur `reader_ctx`
- réserver `writer_ctx` aux transactions d’ingestion, claims et mutations
- garder `service_guard` comme signal de dégradation, mais pas comme seul mécanisme de priorité
- ne pas oublier qu'une partie du MCP reste write-capable, notamment pour les mises à jour `SOLL`/`ZOL`: la cible n'est donc pas "MCP = lecture seulement", mais "MCP read-only séparé du bulk writer, et MCP write interactif priorisé proprement"

### 15. La résilience aux échecs d'écriture est partielle seulement

Constat:

- si `writer_ctx` est occupé, les appels se bloquent sur le mutex; il n'existe pas de timeout ou de file de priorité séparée
- si `BEGIN`, `UPDATE` ou `COMMIT` échouent lors d'un claim ou d'un batch SQL, le code remonte une erreur immédiate
- l'ingestor autonome journalise l'erreur puis continue sa boucle au cycle suivant
- au redémarrage, les fichiers restés `indexing` sont bien remis en `pending`
- en revanche, dans le writer actor, si `insert_file_data_batch` échoue, le batch est seulement loggé en erreur puis vidé
- plus grave, le feedback `FileIndexed` continue d'être émis même si le commit batch a échoué

Impact:

- la résilience au redémarrage existe, mais la résilience "in-process" est incomplète
- un échec d'écriture batch peut laisser des fichiers bloqués en `indexing` jusqu'au redémarrage
- l'observabilité peut devenir mensongère: un fichier peut être annoncé comme indexé alors que la transaction DB a échoué
- il n'existe pas aujourd'hui de retry explicite, de dead-letter queue, ni de requeue immédiat sur erreur de commit batch

Corrective follow-up:

- ne pas émettre le feedback `FileIndexed` tant que le commit batch n'a pas réussi
- requeue ou repasser explicitement en `pending` les fichiers du batch si `insert_file_data_batch` échoue
- ajouter un retry borné avec backoff pour les erreurs d'écriture transitoires
- distinguer clairement les erreurs de contention writer, de commit DB et de corruption plus grave
- prévoir explicitement un chemin d'écriture interactif pour les outils MCP de mise à jour `SOLL`/`ZOL`, distinct du bulk writer d'indexation afin qu'un agent ne soit pas bloqué derrière un lot lourd

### 16. La qualité effective des réponses MCP est hétérogène et insuffisante pour un usage fiable sans supervision

Constat:

- le serveur MCP répond bien et expose ses outils
- `axon_query` donne des résultats utiles sur des symboles ciblés, par exemple `claim_policy`
- `axon_inspect` répond, mais reste pauvre et peu détaillé
- `axon_debug` reste trompeur sur l'état d'indexation: il affiche `48811` "fichiers indexés" alors que le SQL direct ne montrait que `131` fichiers en `indexed`
- `axon_impact` produit une sortie trop bruitée et contradictoire: il annonce un faible rayon d'impact, puis "Aucun résultat trouvé", puis une projection locale saturée de voisins parasites
- `axon_audit` fuit hors du scope demandé: un audit demandé sur le projet `axon` remonte des éléments provenant d'autres dépôts du workspace

Impact:

- la disponibilité du MCP ne suffit pas; sa fiabilité métier reste insuffisante
- un utilisateur ou un agent peut tirer des conclusions fausses sur:
  - la complétion réelle de l'indexation
  - le scope du projet audité
  - le rayon d'impact réel d'un symbole
- en l'état, `axon_query` est le meilleur outil testé, mais `axon_debug`, `axon_impact` et `axon_audit` ne sont pas encore des surfaces de confiance

Corrective follow-up:

- corriger `axon_debug` pour aligner ses compteurs avec la vérité SQL réelle
- corriger le scoping project dans `axon_audit`
- réduire fortement le bruit et les faux voisins dans `axon_impact`
- établir une évaluation systématique de qualité outil par outil avec cas d'usage réels

### 17. Le goulot d'étranglement d'aiguillage DB est confirmé et des sondes supplémentaires sont nécessaires

Constat:

- le goulot d'étranglement critique n'est plus une hypothèse: lectures MCP/SQL, claims d'ingestion et écritures batch convergent encore excessivement vers `writer_ctx`
- ce défaut d'aiguillage peut pénaliser à la fois:
  - la latence du serveur MCP
  - le débit de l'indexation
  - la fidélité des statuts et diagnostics observés
- plusieurs phénomènes restent non maîtrisés dans l'ingestion:
  - backlog `pending` massif sans progression visible cohérente
  - incohérences entre volume du graphe, statuts SQL et diagnostic MCP
  - intermittence de réponse des surfaces pendant la charge

Impact:

- tant que ces phénomènes ne sont pas mesurés plus finement, on ne maîtrise pas réellement le comportement de l'ingestion
- l'architecture et l'observabilité ne permettent pas encore de distinguer clairement:
  - contention DB
  - lenteur de claim
  - lenteur de commit batch
  - latence de lecture MCP
  - divergence des statuts persistés

Corrective follow-up:

- placer plusieurs sondes supplémentaires aux bons endroits du pipeline, au minimum sur:
  - attente d'acquisition de `writer_ctx`
  - durée des claims `BEGIN -> UPDATE -> SELECT -> COMMIT`
  - durée des `execute_batch`
  - temps de réponse MCP par outil
  - temps de lecture SQL read-only
  - transitions d'état `pending -> indexing -> indexed/indexed_degraded/skipped/oversized`
- exposer ces mesures dans une télémétrie canonique exploitable
- ouvrir une tranche d'analyse dédiée pour comprendre les phénomènes d'ingestion non maîtrisés avant toute optimisation agressive

### 18. Le projet `axon` montre un phénomène fort de refile vers `pending` alors que la matière structurelle existe deja

Constat:

- comparaison directe projet `axon`:
  - `504` lignes `File`
  - `130` en `indexed`
  - `374` en `pending`
- comparaison disque/base sur ces `504` chemins:
  - les `130` chemins absents du disque sont tous en `indexed`
  - les `374` chemins encore presents sur disque sont tous en `pending`
- la couche structurelle du projet `axon` est pourtant deja riche:
  - `3732` `Symbol`
  - `3732` `Chunk`
  - `7547` `CONTAINS`
  - `1481` `CALLS`
  - `2` `CALLS_NIF`
- parmi les `374` fichiers `pending` encore presents sur disque:
  - `358` ont deja au moins une relation `CONTAINS`
  - `358` ont deja au moins un `Chunk`
  - seulement `16` sont `pending` sans `CONTAINS`

Exemples forts:

- `/home/dstadel/projects/axon/src/axon-core/src/main_background.rs` est `pending` avec `90` symboles/chunks
- `/home/dstadel/projects/axon/src/axon-core/src/queue.rs` est `pending` avec `52` symboles/chunks
- `/home/dstadel/projects/axon/src/axon-core/src/embedder.rs` est `pending` avec `31` symboles/chunks
- de nombreux artefacts de couverture HTML sous `src/dashboard/cover/` sont eux aussi `pending` avec beaucoup de symboles/chunks

Interpretation provisoire:

- pour le projet `axon`, `pending` ne veut majoritairement pas dire "jamais traite"
- il veut plutot dire "rebasculé en attente alors qu'une verite structurelle a deja ete persistee"
- les `indexed` visibles correspondent ici essentiellement a des chemins historiques absents du disque actuel, pas aux fichiers actuels du repo

Hypothese causale forte:

- la logique de `upsert_file_queries` et de requeue remet massivement en `pending` des fichiers deja materialises
- la DB conserve donc un graphe riche pendant que la table `File.status` raconte une progression tres degradee

Corrective follow-up:

- poser une sonde explicite sur chaque passage `indexed/indexed_degraded/skipped -> pending`
- distinguer les causes:
  - repriorisation
  - changement `mtime/size`
- `needs_reindex`
- invalidation soft / version drift
- recovery apres crash
- nettoyer ou qualifier les chemins historiques absents du disque pour ne pas confondre backlog reel et reliquat historique

Faits discriminants supplementaires:

- `RuntimeMetadata` courant est aligne:
  - `schema_version = 2`
  - `ingestion_version = 3`
  - `embedding_version = 1`
- sur les `504` lignes `File` du projet `axon`:
  - `needs_reindex = false` pour `504/504`
  - `defer_count = 0` pour `504/504`
  - `last_error_reason = NULL` partout

Interpretation affinée:

- le `pending` massif observe sur `axon` n'est pas explique, dans l'etat courant, par:
  - un version drift actif
  - un `needs_reindex` arme
  - une defer fairness debt
  - une erreur explicite persistée
- l'explication la plus plausible reste donc une remise en `pending` par le chemin ordinaire de rescan / upsert, probablement lors de nouvelles insertions scanner sur des fichiers deja materialises

## Current Status

- bootstrap: OK
- runtime start: OK
- dashboard: OK
- SQL: OK
- MCP: disponible, mais qualité des réponses insuffisante sur plusieurs outils critiques
- attente en cours:
  - stabilisation observable de l'indexation initiale
  - clarification de la cohérence entre activité runtime, statuts SQL et diagnostic `axon_debug`
  - compréhension fine des phénomènes non maîtrisés dans l'ingestion

### 19. Décision d'architecture retenue pour limiter le churn d'ingestion

Décision:

- introduire un filtre amont mémoire dérivé de `File`, nommé `FileIngressGuard`
- rôle strict:
  - filtrer scanner/watcher avant écriture dans DuckDB
  - éviter les `upsert` redondants et les requeues silencieux
- ne pas lui donner:
  - l'autorité de claim
  - l'autorité de priorité
  - l'autorité de mutation de statut
- DuckDB reste la seule vérité canonique

Points verrouillés:

- pas de priorisation canonique dans le guard
- pas de favoritisme canonique du repo `axon` par rapport aux autres projets
- premier signal de décision = `path + mtime + size`
- pas de hash fichier dans le MVP
- le guard doit `fail-open`
- mise à jour du guard uniquement après succès canonique DB

Artefacts créés:

- `docs/plans/2026-04-02-file-ingress-guard-design.md`
- `docs/plans/2026-04-02-file-ingress-guard-implementation-plan.md`

### 20. Point à investiguer ensuite: relâchement mémoire après pics d'indexation

Constat utilisateur:

- Axon a pu monter autour de `16 GB` de RAM
- il ne faut pas "réduire la voilure" fonctionnelle
- mais il faut comprendre si une partie importante correspond à du cache DB / working set libérable

Interprétation:

- il faut distinguer:
  - mémoire utile et durable du runtime
  - working set DuckDB / cache pages
  - buffers temporaires d’ingestion
  - mémoire du worker sémantique
- la prochaine investigation mémoire ne doit pas être un simple throttling
- la bonne question est: "qu’est-ce qu’Axon peut relâcher proprement après un pic sans perdre sa vérité ni casser le débit futur ?"

Follow-up:

- ajouter une tranche dédiée d’analyse mémoire runtime
- vérifier les mécanismes DuckDB / WAL / checkpoint / cache qui peuvent être relâchés
- mesurer ce qui reste stable après quiescence
- décider si Axon a besoin d’un mécanisme explicite de purge / compactage / relâchement du working set

### 21. Investigation mémoire: ce qu'Axon fait aujourd'hui ne permet pas de faire redescendre explicitement le RSS

Constats locaux confirmés:

- Axon utilise aujourd'hui l'allocateur système par défaut, pas `jemalloc`, dans `src/axon-core/src/main.rs`
- le runtime ne fait actuellement:
  - ni `malloc_trim`
  - ni réglage explicite `DuckDB memory_limit`
  - ni réglage `temp_directory`
  - ni réglage `max_temp_directory_size`
  - ni instrumentation fine `RssAnon` / `RssFile`
- un `CHECKPOINT` est bien exécuté au bootstrap DB dans `src/axon-core/src/graph_bootstrap.rs`
- ce `CHECKPOINT` de boot ne constitue pas, à lui seul, un mécanisme de relâchement mémoire après pic
- la base `.axon/graph_v2` pèse actuellement environ `5.5G` sur disque

Documentation officielle DuckDB:

- `memory_limit` existe, mais DuckDB documente explicitement que cette limite ne couvre que le `buffer manager`
- la consommation réelle peut depasser cette limite car certaines structures vivent hors `buffer manager`
- DuckDB documente aussi:
  - `temp_directory`
  - `max_temp_directory_size`
  - `duckdb_memory()`
  - `duckdb_temporary_files()`
  - `PRAGMA database_size`
- DuckDB indique que le `buffer manager` garde des pages en cache entre requêtes tant que cet espace n'est pas requis ailleurs ou jusqu'à fermeture de la base
- `CHECKPOINT` et `checkpoint_on_shutdown` servent surtout la persistance/WAL et la compaction disque, pas une baisse garantie et immédiate du RSS

Retours communauté / pratique prod:

- un RSS élevé après pic n'indique pas forcément une fuite DuckDB
- les causes récurrentes observées sont plutôt:
  - rétention allocateur
  - pages/cache DuckDB encore résidentes
  - structures allouées hors `buffer manager`
  - fragmentation et arènes multi-thread
- les mitigations citées en pratique sont:
  - `memory_limit` plus conservateur
  - `temp_directory` explicite pour permettre le spill
  - réduction des threads sur gros workloads
  - instrumentation via `duckdb_memory()`
  - allocateur plus adapté (`jemalloc`) ou trim explicite côté glibc

Lecture opératoire actuelle:

- on ne sait pas encore si les pics Axon viennent majoritairement de:
  - `RssAnon` (heap/allocateur/process)
  - `RssFile` (mappings fichiers/page cache)
  - worker sémantique ONNX
  - working set DuckDB
- tant qu'on ne distingue pas `RssAnon` et `RssFile`, changer d'allocateur serait prématuré

Décision provisoire:

- ne pas "réduire la voilure" fonctionnelle pour l'instant
- commencer par instrumenter correctement la nature du RSS
- ensuite seulement tester, dans cet ordre:
  1. visibilité `RssAnon` / `RssFile` + métriques DuckDB
  2. réglages DuckDB explicites (`memory_limit`, `temp_directory`, `max_temp_directory_size`)
  3. `malloc_trim(0)` ou équivalent après gros pics si le problème est majoritairement `RssAnon`
  4. éventuellement retour vers `jemalloc` ou autre allocateur si la mesure le justifie et si l'axe FFI/ONNX reste stable

Corrective follow-up:

- ajouter des sondes runtime pour:
  - `RssAnon`
  - `RssFile`
  - `RssShmem`
  - taille DB/WAL
  - `duckdb_memory()`
  - `duckdb_temporary_files()`
- vérifier si un `checkpoint_on_shutdown` ou un `CHECKPOINT` périodique réduit au moins le WAL, sans promettre de baisse RSS
- ouvrir une tranche distincte "relâchement mémoire post-pic" avant tout changement d'allocateur

Sources externes utilisées:

- DuckDB Pragmas: https://duckdb.org/docs/current/configuration/pragmas
- DuckDB Memory Management: https://duckdb.org/2024/07/09/memory-management
- DuckDB Out of Memory guide: https://duckdb.org/docs/stable/guides/troubleshooting/oom_errors.html
- DuckDB Limits: https://duckdb.org/docs/stable/operations_manual/limits
- DuckDB checkpoint docs: https://duckdb.org/docs/current/sql/statements/checkpoint
- DuckDB reclaiming space: https://duckdb.org/docs/current/operations_manual/footprint_of_duckdb/reclaiming_space
- DuckDB performance environment: https://duckdb.org/docs/stable/guides/performance/environment.html
- GNU `malloc_trim(3)`: https://man7.org/linux/man-pages/man3/malloc_trim.3.html
- GNU allocator manual: https://sourceware.org/glibc/manual/2.27/html_node/The-GNU-Allocator.html
- WSL config: https://learn.microsoft.com/windows/wsl/wsl-config

### 22. Observabilité mémoire runtime implémentée

Constat:

- la tranche read-only d'observabilité mémoire est maintenant en place
- `RuntimeTelemetry` expose désormais:
  - `rss_bytes`
  - `rss_anon_bytes`
  - `rss_file_bytes`
  - `rss_shmem_bytes`
  - `db_file_bytes`
  - `db_wal_bytes`
  - `db_total_bytes`
  - `duckdb_memory_bytes`
  - `duckdb_temporary_bytes`
- `axon_debug` agrège maintenant:
  - volume graphe
  - backlog réel
  - mémoire runtime détaillée
  - stockage DuckDB
  - mémoire DuckDB agrégée

Impact:

- on peut enfin distinguer, dans les prochaines observations réelles, si les pics viennent plutôt:
  - du heap/process
  - du cache fichier
  - du working set DuckDB
  - du spill temporaire

Limite restante:

- cette tranche n'active encore:
  - ni purge mémoire
  - ni `memory_limit` explicite
  - ni `temp_directory`
  - ni changement d'allocateur

### 23. Première causalité persistée des retours vers `pending`

Constat:

- la table `File` porte maintenant une colonne `status_reason`
- premières causes persistées couvertes:
  - `metadata_changed_scan`
  - `metadata_changed_hot_delta`
  - `recovered_interrupted_indexing`
  - `needs_reindex_while_indexing`
  - `soft_invalidated`
  - `manual_or_system_requeue`
  - `oversized_for_current_budget`

Impact:

- la base ne dit plus seulement "pending", elle commence à dire pourquoi
- cela améliore fortement la forensique du churn observé sur le premier run

Limite restante:

- toute la machine d'état n'est pas encore qualifiée
- il faut encore couvrir exhaustivement les transitions restantes et exposer ces causes dans les vues opératoires

### 24. Les vues MCP expliquent maintenant le backlog et la complétude projet

Constat:

- `axon_debug` expose maintenant les causes dominantes du backlog global
- les outils MCP scope-projet exposent une note de complétude du scope demandé

Contenu désormais visible:

- fichiers terminés / fichiers connus
- backlog visible
- répartition `pending` / `indexing`
- causes backlog dominantes si présentes

Correction importante:

- `axon_audit` et `axon_health` ne comptent plus un projet via `project_slug OR path LIKE`
- le scope repose maintenant sur `project_slug` uniquement

Impact:

- un agent MCP peut désormais savoir rapidement si sa réponse projet est quasi complète ou très partielle
- l'opérateur voit enfin une explication synthétique du backlog sans requête SQL manuelle

Limite restante:

- ces vues décrivent mieux le churn mais ne ferment pas encore toute la causalité de la machine d'état
- il faut encore mesurer ces notes sur un vrai run long pour distinguer backlog vivant vs reliquat historique

### 25. Les transitions de scheduling portent maintenant une cause opératoire

Constat:

- une claim effective vers `indexing` pose maintenant `status_reason = 'claimed_for_indexing'`
- un déferrement volontaire par le scheduler pose maintenant `status_reason = 'deferred_by_scheduler'`

Impact:

- on peut enfin distinguer:
  - un fichier simplement claimé pour exécution
  - un fichier en backlog différé pour raisons de scheduling/fairness
- cela réduit encore l'ambiguïté du `pending` observé pendant les runs longs

Limite restante:

- toute la machine d'état n'est toujours pas complètement couverte
- il reste à qualifier d'autres transitions silencieuses avant de déclarer la causalité fermée

### 26. Le succès complet n'est plus un statut final muet

Constat:

- un commit complet réussi pose maintenant `status_reason = 'indexed_success_full'`

Impact:

- `indexed` n'est plus un état final sans interprétation
- on distingue mieux:
  - succès complet
  - succès dégradé
  - skipped
  - oversized

Limite restante:

- la machine d'état est mieux qualifiée, mais il reste encore des transitions rares à inventorier avant fermeture totale

## Follow-up Corrections to Plan

Si la fin d'indexation initiale ne peut pas être constatée proprement sans heuristique, ouvrir une tranche corrective sur:

1. émission canonique d'un événement bridge de fin de scan initial
2. exposition d'un état `indexing_complete` ou `initial_scan_complete` sur la télémétrie runtime
3. documentation opérateur WSL plus explicite sur la différence entre:
   - bootstrap
   - démarrage runtime
   - scan initial
   - watch continu
