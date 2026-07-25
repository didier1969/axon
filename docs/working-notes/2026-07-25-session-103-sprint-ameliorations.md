# Session 103 — 2026-07-24/25 · Sprint « améliorations »

> Note d'AUDIT (append-only). Reprise canonique = session_pointer **`CPT-AXO-052`**.
> Ici = le narratif « comment on y est arrivé » + les pièges rencontrés.

## Cadre

Directive opérateur : *« go all sauf 902237 »* + *« analyse les usages des LLM et si de
nouvelles difficultés ou demandes »*. 902237 écarté parce que sa validation exige une
charge GPU, et le driver `nvlddmkm` n'est pas réparé. Tout le sprint a donc été conçu
**CPU/PG-safe**.

4 agents de scoping lecture-seule (Axon-first) ont ancré le plan — et **deux de leurs
trouvailles ont renversé le brief** :

1. **902185 était débloqué depuis 9 jours.** Je m'apprêtais à implémenter la complexité
   cyclomatique `.lll` dans llmlang. Inutile : **LLL l'a livrée le 15/07** (REQ-LLL-172,
   commit `e525008` — vérifié live : `ledger.lll` → `cc=3`), et le pont côté axon-core est
   **entièrement générique** (`properties["cyclomatic_complexity"]` string → `graph_ingestion`
   → `snapshot.complexity_of` → god-objects). **Zéro code AXO.** La « mailbox LLL muette »
   de la mémoire était périmée.
2. **902234 VOLET 2 était déjà ~80 % fait.** La REQ affirmait que le worker query-embed
   n'était pas couvert par un idle-drop ; le code dit le contraire (`query_worker_loop_lane`,
   `AXON_QUERY_EMBED_IDLE_SECS`, défaut 1200 s). Ma propre rédaction de REQ était une
   analyse partielle → triage CPT-AXO-025 #1, pas un bug Axon.

## Livré

### 902233 slice A — graceful shutdown (`b5674062`)
La **root cause du brain-zombie** n'était pas dans le promote : le keep-alive de `boot()`
(`runtime_boot.rs`) était un future qui **ne se résout jamais** (`pending::<()>()` ou la
boucle d'accept télémétrie). Au SIGTERM d'un restart process-compose, le process ne
déroulait pas → `Terminating` jusqu'au SIGKILL → aucun brain neuf → panne totale.

Fix : les deux chemins keep-alive **racent** un future SIGTERM/SIGINT (`tokio::select!` +
`shutdown_signal()`). Symétrique brain + indexeur (même `boot`) — sur l'indexeur, Drop a
désormais une chance de relâcher la session GPU au stop.

**Preuve behaviorale** : SIGTERM direct → sortie **déterministe en 1,98 s**. (Sans handler,
SIGTERM par défaut tue instantanément ; les ~2 s SONT le handler qui intercepte et déroule.)

### 902233 — mesure, et la décision de NE PAS coder B/C (`272ff9ea`)
Le hammer test (`hammer_mcp_during_boot.sh`, jusque-là non tracké) mesure ce qui compte :
la fenêtre client réelle, par de **vrais** `tools/call`, pas `/readyz`.

Restart brain, dev `brain_only`, binaire A-fixé :

| t | readyz | mcp | lecture |
|---|---|---|---|
| t+000..002 | 200 | PASS | avant |
| **t+003** | refused | **FAIL** | OLD brain sort (graceful) |
| t+005 | refused | PASS | NEW brain sert déjà le MCP |
| t+008+ | 200 | PASS | tout vert |

**Downtime client = 1 FAIL (~1-2 s), GAP `/readyz`-ment = 0 s.**

⇒ **A atteint déjà le critère &lt;2 s**, donc **B (retry/backoff `-32000`) et C (`/readyz`
honnête) ne sont PAS justifiées** : le gap qu'elles corrigent ne s'est pas manifesté
(`readyz` suivait la vraie dispo : refused→refused→200). C'est exactement la logique
« escalader seulement si le gap mesuré dépasse le critère » que le REQ posait — appliquée
au lieu d'être contournée. Consigné dans le corps du REQ via `append_section` (le
patch-write que OPV réclamait — dogfood).

Reste **E** (découpler le warm indexeur du brain-serving) pour le cas **promote**-300 s où
l'indexeur GPU cold-start bloque le health-gate : à mesurer **sur un vrai promote**, pas un
restart brain, et en préservant l'auto-rollback indexeur (REQ-902165).

### 902192 slice S4-minimal — viz `/wiring` (`9b8df19a`)
Dernier reste d'une umbrella à ~90 % livrée. LiveView non-canonique (PIL-AXO-009) qui lit
`wiring.data.orphans[]` + compteurs, avec sélecteur de projet re-scopé serveur. Mirror exact
du pattern `DriftHeatmapLive` (mount → Task supervisée → `McpClient.call_tool` → `Nav.shell`) —
aucun réflexe JS (pas de fetch/setInterval client).

**Vérifié navigateur réel** (pas un 200) : 20 orphelins AXO (`reset_for_tests` 18 test-callers,
`unset` 15, `persist_contract` 14…), sélecteur 40 projets, badges test-only/isolated, légende,
**zéro erreur console**.

## Analyse usages LLM (télémétrie réelle : 87 772 appels / 720 h)

Le résultat le plus utile est un **diagnostic qui invalide l'alerte** :
`soll_acyclic_audit` était à **100 % d'échec (82/82)**. L'outil n'est **pas cassé** — avec
`project_code` il répond parfaitement (0 SCC, 1512 nœuds). Il **exige** l'arg explicite,
alors que `query`/`inspect` l'**auto-résolvent du cwd** : les LLM l'omettent → échec
systématique. Et c'est une **classe** : `soll_work_plan`, `structural_health_index`, et
même `soll_manager create` — ironie vécue en direct, ma création de REQ a échoué pour
cette exact raison pendant que je loggeais la friction.

Loggé : **REQ-902239** (project_code, P2) · **REQ-902240** (`query` dupliqué — observé 2× moi-même,
P2) · **REQ-902241** (`practice_put` sans supersede, P3) + CPT-90059/60/61.

Signaux de fond **non traités** (surfacés) : `sql` = **68 % de tout le trafic** (les LLM
tombent massivement en SQL brut = ergonomie de découvrabilité) ; `wiring`/`orphan_clusters`/
`structural_health_index` sous-utilisés (&lt;48 appels) ; l'ask OPV **volet-2 lignage de
DONNÉES** n'a **aucun REQ** → à logger comme étude ou rejeter explicitement.

## Pièges rencontrés (le vrai contenu de cette note)

- **⛔ Incident que j'ai causé.** `axon-dev stop --hard` pendant que le live tournait : le
  reap axonctl (supervisor-tree) a mis l'indexeur **live** en `Disabled` → live DEGRADED.
  Réparé (`POST :8080/process/start/axon-indexer`, up en 3 s). Leçon : **jamais `--hard`
  quand le live tourne**, et **vérifier `axon-live status` après CHAQUE op runtime dev**.
  Devenu practice 369 + garde-fou du plan.
- **Le même symptôme revient après un reboot WSL** : le self-heal relance le brain **seul**,
  l'indexeur reste `Disabled` → DEGRADED. Vu le 13/07 et le 25/07. Même fix.
- **`rescan_project` n'a PAS de mode graph-only** : `full=false` saute les fichiers inchangés
  (cache content-hash), `full=true` re-parse **et re-embed** (GPU). Le plan promettait
  « zéro GPU » pour le rescan LLL — c'était faux, j'ai **différé** l'item plutôt que de
  prendre le risque driver. Practice 401.
- **Le dashboard live tourne depuis les sources en prod, sans hot-reload** : `/wiring`
  n'apparaissait qu'après `POST :8080/process/restart/dashboard` (recompile ~36 s, zéro GPU).
- **`practice_put` est LENT** (`embed=deferred` sur brain_only) : mes timeouts curl à 25 s
  le faisaient échouer silencieusement → 45-60 s nécessaires. Ne pas conclure « outil cassé »
  sur un timeout client.

## Correction que je me dois

J'ai affirmé que le graceful shutdown « dé-risquait le reboot DDU » de l'opérateur. **Faux** :
un reboot OS force-kill tout, il n'y a pas de zombie à éviter là. Le fix protège les
**restarts/promotes process-compose**, ce qui est le scope réel de 902233.

## État machine (BSOD) — la dormance trompe

Stats Windows (journal d'événements, 60 j) : **15 BSOD, dont 87 % `0x133`+`0x9f`** = classe
GPU-driver-timeout (le WinDbg n'était pas un one-off). Le motif régulier démarre le **11/06**,
soit **6 jours après la pose du driver du 05/06** ⇒ c'est **cette installation** qui a
introduit l'instabilité (donc clean-install DDU, pas un rollback « après le 25/06 »).

Après le reboot du 25/07 : `nvidia-smi` = **610.52** = exactement le WDDM `32.0.16.1052`
déjà relevé ⇒ **driver inchangé, DDU pas encore effectif**. HAGS bien désactivé ; VRR laissé
ON (écran interne only → hors cause documentée). **Le calme depuis le 14/07 est une dormance
— la charge GPU a disparu, pas le bug.** Toute session GPU lourde peut le réveiller.
