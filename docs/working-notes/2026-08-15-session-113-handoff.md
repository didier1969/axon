# Session 113 — 2026-08-15 — « les mesures qui mentent » + incident dxgvmb en clôture

> ⚠️ **Ce handoff est écrit en FALLBACK FICHIER.** Le brain MCP est tombé pendant la
> clôture (wedge `dxgvmb`, voir §4). Les REQ SOLL rédigés avant 19:56 sont bien en base ;
> **deux** ne le sont pas et figurent en §5 pour être recréés à la reprise.
> Canal canonique de reprise = ce fichier + `git log`, jusqu'à ce que MCP revienne.

---

## 1. Le fil rouge de la journée

Une seule forme, dix fois. **Aucun de ces défauts ne plante ; chacun rend une réponse
plausible qui n'est pas la vraie.** C'est ce qui les rend chers : on ne les trouve qu'en
vérifiant ce qu'un chiffre compte réellement, ou en cassant exprès ce qu'on vient de
réparer pour exiger de voir le rouge.

**Deux de ces défauts sont de moi, le jour même** — et deux de mes propres tests étaient
incapables de rougir. Ce n'est pas une histoire de vieux code.

## 2. Livré — 9 commits, tous poussés

| Commit | Objet |
|---|---|
| `70a9ac79` | `soll_work_plan` classait les REQ **pour leur inachèvement** : 69 % des points d'un REQ venaient de « pas de preuve » ; `status=current` valait 0 (REQ-902295) |
| `83cee44f` | `ist.Chunk.created_at_ms` + la phrase « DBQ-A claim feeder » **encore imprimée** à chaque appel de `diagnose_indexing` (REQ-902260) |
| `a8439770` | **Défaut que j'avais livré une heure plus tôt** : le bonus d'engagement tombait aussi sur les Decisions, où `current` veut dire *en vigueur* (REQ-902295) |
| `ff3b99cf` | Suite non déterministe (verrou d'env opt-in) + `sql` jetait la cause réelle de son erreur (REQ-902326, 902323) |
| `63b15813` | Le **chemin d'échec** du promote était cassé : fonction appelée avant définition ; un commentaire entre backticks **exécuté** à chaque promote (REQ-902327) |
| `e5a39851` | Un `lock timeout` est rejouable et **ne dit rien** du schéma (REQ-902328) |
| `572e89b8` | Le filet du gate annonçait une restauration **jamais vérifiée** (REQ-902293) |
| `0301a2fa` | `practice_card` : une **erreur de type** rendue comme la valeur 0 (REQ-902325) |
| `b193e3c3` | `schema_overview` cachait **36 des 61 tables** du produit (REQ-902329) |
| `b682671e` | Un chemin CLI retiré, décrit comme vivant en docstring (REQ-902271) |
| `dcaeac73` | `tech_debt_inventory` : **11 résidus annoncés, 0 réel** (REQ-902331) |

`soll_validate` = 0. Arbre propre. `origin/main` = `dcaeac73`.

## 3. Les cinq leçons qui valent pour la suite

1. **Vérifier ce qu'un chiffre compte AVANT de décider.** `mean trust 0.00` n'était pas
   un problème d'agrégation mais de décodage : `try_get::<i64>` sur un `double precision`
   → `unwrap_or(0)`. Le repli `unwrap_or(500)` de l'appelant était **inatteignable**,
   la fonction ne renvoyant jamais d'erreur. *Une signature sans canal d'erreur convertit
   chaque échec en un chiffre que quelqu'un lira comme une mesure.*
2. **Falsifier chaque garde avant de la committer.** Deux de mes assertions étaient
   vacuous — compteur de stub dans un sous-shell, puis stub écrivant sur un stdout que
   l'appelant redirige vers `/dev/null`. Les deux fois, c'est le contrôle négatif qui l'a
   montré : neutraliser le correctif laissait le test **vert**.
3. **Une garde qui scanne des sources lit du CODE, jamais de la documentation.**
   Quatre auto-matchs en une journée : mes gardes se déclenchaient sur la prose qui
   documente le défaut qu'elles interdisent.
4. **Mesurer avant de généraliser.** La garde de classe « verrou d'env » paraissait juste
   jusqu'au comptage : **370 contrevenants, sur-approximation ×40**. Retirée. La classe ne
   se ferme pas par une garde statique mais en **supprimant les globales**.
5. **Un outil bat un grep.** Mon `grep`, restreint à `*.rs` sous `src/axon-core/`, a
   conclu à tort « uniquement des commentaires » ; `detect_remnants` a sorti trois scripts
   Python. (Le même outil rendait par ailleurs 11 faux positifs — les deux sont vrais.)

## 4. ⛔ INCIDENT EN COURS — wedge `dxgvmb`, MCP DOWN

### Faits
```
Dl  1318848 (axon-brain)    tokio-rt-worker  dxgvmb_send_sync_msg
Dl  3968338 (axon-indexer)  tokio-rt-worker  dxgvmb_send_sync_msg  ×2
Dl+ 3979649 nvidia-smi                       dxgvmb_send_sync_msg
```
- `axon-brain` : **`Terminating`**, `/readyz` = 000 (timeout 8 s). **MCP inutilisable.**
- `axon-indexer` : Running/Ready mais **`restarts=3/3`** — budget épuisé, jamais régénéré.
- Charge hôte **31**. `postgres` du projet **OPV tué 2× par SIGKILL** (19:38, 19:49).

### Chaîne
Trois morts de l'indexeur avant le brain, toutes par **SIGSEGV dans `libnvinfer`**
(TensorRT) : 19:19:07 et 19:46:01 capturés par le noyau, plus la capture de crash WSL.
**Ce n'est pas un OOM** (le seul du jour date de 00:01 et visait un autre processus) **ni
le changement de schéma** du jour (6 h de fonctionnement propre après son application).

Puis le canal vmbus GPU s'est jammé, et un thread du brain s'y est bloqué en D
ininterruptible — donc le processus ne peut pas finir de sortir, d'où `Terminating`
perpétuel. Mécanique identique à REQ-902271 (session 111).

### Reprise — ORDRE STRICT, décision opérateur
1. **`wsl --shutdown`** côté Windows. **Seul remède connu** d'un `dxgvmb` jammé : SIGKILL
   est inopérant sur un thread D. ⚠️ Ferme TOUTES les sessions Windows de l'opérateur.
2. Redémarrer WSL, puis `./scripts/axon --instance live start --indexer-full`.
3. ⚠️ Le start standing est `brain_only` → **`POST :8080/process/start/axon-indexer`** ensuite.
4. Vérifier : 4 rôles Ready · `restarts=0` · `ps -eLo stat,wchan | grep dxgvmb` **vide** ·
   `pgrep -c rustc` = 0 · charge < 10.
5. **Ne PAS lancer de requête `semantic`** avant que le canal soit libre (chaque
   query-embed ajoute un thread D).

### Ce qu'il ne faut PAS faire
- Pas de `wsl --shutdown` automatique : c'est une décision opérateur (REQ-902271).
- Pas de bascule CPU-embed : **refusée par directive opérateur**, ne pas la re-proposer.
- Pas de promote avant que le canal soit libre ET l'hôte au repos.

## 5. ⚠️ DEUX REQ À RECRÉER — écrits mais NON persistés (MCP tombé à 19:56)

### A. « PROMOTE EN ATTENTE » (P1, REFINES REQ-AXO-902256)
HEAD `dcaeac73` = origin/main ; live `v0.8.0-1482-gff3b99cf` ; **8 commits d'avance**
(3 shell **déjà actifs**, 5 Rust invisibles).

Ce qui reste **faux sur le live** faute de promote :

| Surface | Dit aujourd'hui | Vérité |
|---|---|---|
| `practice_card` | `mean trust 0.00` | 0,53 (REQ-902325) |
| `schema_overview` | 25 tables | **61** (REQ-902329) |
| `tech_debt_inventory` | 11 résidus | **0** (REQ-902331) |

`soll_work_plan` (REQ-902295) est **déjà live** depuis 13:32, vérifié sur deux tenants.

**Refusé le 2026-08-15 à 19:52**, contre la consigne de 19:09, parce que les conditions
avaient changé entre les deux : indexeur en segfault TensorRT, `restarts=3/3`, charge 28.
Le step 2d redémarre l'indexeur LIVE — le faire alors reproduisait l'échec du matin, dont
le message dit mot pour mot *« re-run on an idle host »*.

**Bénéfice collatéral** : le promote remet `max_restarts` à 0/3 — argument pour le faire
dès que l'hôte est calme, pas pour le faire maintenant.

**Après le promote, ne pas oublier** :
1. `detect_remnants reset_baseline=true` — sinon la progression de la dette se mesure
   encore contre le chiffre faux ;
2. clore les critères « EN ATTENTE DE PROMOTE » de REQ-902325 et REQ-902329.

### B. Compléter REQ-AXO-902332 (déjà créé) — rien à recréer, pour mémoire
Le REQ existe en base. Sa **vraie leçon** : la sortie de l'indexeur **n'est capturée nulle
part** (`process-compose` ne journalise que le cycle de vie, aucun `log_location`,
`axon.vectorworkerfault` ne contient que des lignes de démo). La cause n'a été trouvée que
parce que le **noyau** l'a journalisée. *Un incident dont le diagnostic dépend d'un log
système hors du produit n'est diagnosticable ni par l'opérateur ni par un LLM.* À traiter
AVANT la récurrence TensorRT elle-même.

## 6. État SOLL à la reprise

Créés cette session : `DEC-AXO-901668` (deux nombres pour deux questions) ·
`DEC-AXO-901669` (le gate de cycle de vie reste à chaque release) · REQ **902323, 902325,
902326, 902327, 902328, 902329, 902330, 902331, 902332**.

Fermés : **902295** (`delivered`, 8 critères, prouvé sur AXO **et** SWT) · **902271**
(point 2 caduc par mesure) · **902293** (arbitrage tranché par `DEC-AXO-901669`).

Ouvert et non commencé : **902330** — demande client BKB (l'IST est aveugle au câblage
Odoo ; `anomalies` y est inutilisable). **Bloqué par conception** : j'ai demandé à BKB un
jeu de cas-tests réels, attendus **avant** le code, et la réponse est partie
(`msg-65ee6b6c126baee35e83451f`).

## 7. Trois next-actions

1. **`wsl --shutdown`** puis la séquence de reprise du §4. Rien d'autre n'est possible
   tant que le canal GPU est jammé.
2. **Promote** dès l'hôte au repos (§5A), puis `reset_baseline` et clôture des deux
   critères en attente.
3. **REQ-902332** — capturer stdout/stderr par rôle. C'est le préalable à tout diagnostic
   futur, et la journée vient de montrer ce qu'il coûte de ne pas l'avoir.
