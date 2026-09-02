# Session 136 (2026-09-02) — la panne que la demande d'un tenant a déterrée

> Note de travail, append-only. **La SOLL fait foi** : l'état vivant est dans `CPT-AXO-052`,
> quatre dernières sections. Cette note garde le *raisonnement*, pas l'état.

## Ce qui a été livré

`26c5dd4c` — trois surfaces qui affirmaient plus qu'elles ne savaient (`REQ-AXO-902583`
frictions DOC b/c, `REQ-AXO-902588`). Porte `GUI-AXO-1034` : **2112 passed / 0 failed**.
Promote réussi vers `v0.8.0-1701-g26c5dd4c`, **coupure client 44 s**.

Le fait le plus utile du promote : `qualify_mcp` est passé en **13 s** là où il échouait une
fois sur deux. La cause n'était ni la machine ni le candidat — la porte n'échantillonnait la
latence **qu'une fois par outil** et tombait sur les 1,7 % d'appels bloqués. `REQ-AXO-902589`
juge désormais sur la médiane de N appels. Cinq échecs de promote consécutifs expliqués par
une ligne de statistique.

## Le fil conducteur : trois diagnostics faux avant le bon

Cette session a produit **trois hypothèses réfutées**, et elles valent plus que les conclusions
parce qu'elles évitent de re-suivre les mêmes pistes.

### 1. « Le test rouge accuse mon correctif »

`test_project_status_reports_delta_vs_previous_snapshot` rougissait dans la suite complète.
Le diagnostic A/B a rendu `rc=101` sur la branche A — que j'ai failli lire comme un test rouge.
**`cargo` rend le même 101 pour une compilation cassée et pour un test qui échoue.** La branche
A n'avait jamais tourné (`as_str()` sur un `&str`, instable). Verdict vide, pas négatif.

Une fois compilé : vert seul avec les modifications, vert seul sur HEAD, rouge en suite.
Trois faits qui, ensemble, **innocentent le code de production** et accusent une interaction
entre tests parallèles via un état global du processus. Tracé `REQ-AXO-902593` — et le remède
était déjà dans le même fichier, appliqué par le test voisin depuis `REQ-AXO-901721`.

### 2. « Le second test rouge est une régression »

Non : il était **périmé**. `create_nomme_la_relation_qu_il_a_substituee_au_lieu_de_la_nier`
exigeait encore `BLOCKED_BY` dans le texte alors que `REQ-AXO-902588` avait substitué `TARGETS`
— ce que ses **propres assertions structurées affirmaient trois lignes plus haut**. Le symptôme
d'un test périmé est là : ses assertions se contredisent entre elles.

Corrigé, plus une assertion négative : voir revenir `BLOCKED_BY` serait la vraie régression.

### 3. « Le drain et le compteur emploient deux définitions de *pending* »

Séduisant, et **faux**. Le `GROUP BY` croisant `Chunk.embed_status` et la présence réelle dans
`ChunkEmbedding` rend exactement deux classes parfaitement cohérentes : `embedded/true = 508 607`,
`pending/false = 29 492`. Le drain **voit** le stock. La cause était ailleurs.

## ⭐ La découverte : une panne de parc, trouvée en répondant à un tenant

VPC demandait « la voie canonique de requeue » pour leur `963/1773`. **La question était mal
posée** — leur stock était sain, il n'y avait rien à requeue. Mais la vérifier a montré :

**29 483 chunks sans embedding sur 8 tenants.** NEX à 1,18 % de couverture, SNN à 1,13 %,
LLL à 67 %, APS 4 515 chunks en attente, AXO nous-mêmes 3 691.

Prouvé : `vector_workers` effective **0** contre target 5 · `vector_workers_started_total = 0` ·
voie bloquée en `starting` · `b2_pressure = not_armed`. Le stock est visible, le drain le lit,
**personne ne le consomme**. Ce n'est pas une file vide, c'est une porte d'admission fermée.

Trois portes peuvent l'éteindre (`vector_control.rs:1278-1300`), et `allowed_gpu_vector_workers()`
ne rend **jamais** 0 (1, 2 ou 6) — le zéro vient donc forcément de l'amont. **Laquelle : non
tranché.** Le journal de l'indexeur ne trace aucune décision d'admission, et c'est le premier
critère d'acceptation de `REQ-AXO-902597` : sans cette trace, la panne se rediagnostiquera à
l'aveugle à chaque récurrence.

Second défaut, de la classe `MIL-AXO-054` : **`diagnose_indexing` répond `no_blocker_detected`**
sur ce système.

Élément à ne pas perdre : la boucle de reconcile a été **retirée** avec le sous-système sleep/wake
(`REQ-AXO-902036`, documenté verbatim dans `embedder/lifecycle.rs:13-16`), et
`select_chunks_needing_embedding` n'a plus **aucun appelant de production** — vérifié par deux
oracles indépendants, `inspect` et `grep`, parce que l'index d'AXO est `degraded` et qu'un
« 0 appelant » pouvait être un faux négatif.

## Ce que la session dit de notre méthode

**Quatre défauts sur cinq étaient les miens, et aucun n'a été trouvé par relecture.**
Compilation cassée lue comme un test rouge · test périmé lu comme une régression · alerte
tenants ciblée sur le pourcentage alors qu'APS avait le second volume · solution poste
branchée sur un raccourci que l'opérateur n'utilise pas.

Le seul outil qui les a tous attrapés est le **contrôle qui sait échouer** : relire la cible
plutôt que le code de retour, mesurer chez soi avant de relayer le chiffre d'un tiers, exiger
un rouge avant de croire un vert.

Corollaire mesuré hors Axon, sur le poste : `ydotool` rend `0` dès que l'événement est **émis**,
jamais qu'il a **atteint** une fenêtre. Le script disait `select ok` puis `rc=0` pendant que rien
n'était collé. Seule la relecture d'une fenêtre cible a tranché — et donné le seuil réel
(0,45 s échoue, 0,80 s passe).

## Pratiques déposées (scope `*`)

`2068` `2069` `2070` `2071` `2072` `2073` `2075` `2076` `2077` `2078` `2080` `2081`.

Les trois qui resserviront le plus :
- `2069` — rouge en suite + vert seul des deux côtés ⇒ interaction entre tests, pas le code ;
- `2078` — un injecteur d'entrée rend 0 quand l'événement est émis, pas atteint ;
- `2081` — compteur d'attente à 0 avec stock non nul ⇒ chercher le **consommateur**, pas la file.

## Ouvert à la clôture

`REQ-AXO-902597` (P0, la panne de parc) · `REQ-AXO-902594` (P1, DOC — le brain cesse d'accepter
sans cesser de servir) · `REQ-AXO-902595` (P1, CSC — `done` compte des présences, pas des
satisfactions) · `REQ-AXO-902593` (P2) · `REQ-AXO-902547` (brain 2 GiB).

⛔ **Ne pas redémarrer l'indexeur ni promouvoir** : VPC intervient sur les cgroups, et un
redémarrage masquerait la cause de `902597` au lieu de la fermer.
