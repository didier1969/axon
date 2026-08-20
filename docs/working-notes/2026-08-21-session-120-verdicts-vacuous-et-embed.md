# Session 120 — 2026-08-20/21 — « aucun verdict sans dénominateur » + fin de l'incident embed

Audit-only, append-only. Canonique = `CPT-AXO-052` + SOLL. Live à la clôture : `v0.8.0-1532-gbb4119b3`.

---

## 1. Le fil conducteur : un outil qui mesure autre chose que ce qu'il annonce

Trois tenants (APS, NEX, OPV) et VPC ont signalé des défauts pendant la session. En les
mettant côte à côte, **la moitié décrivaient le même mécanisme** : un verdict positif
qui ne dit pas sur quoi il a porté.

| Outil | Ce qu'il rendait | Ce que ça signifiait vraiment |
|---|---|---|
| `architectural_drift` | ✅ aucune dérive | 0 symbole apparié au préfixe |
| `detect_remnants` | 0 résidu | 4 rulesets, tous internes AXO : rien cherché |
| `b3_health` | HEALTHY | surveille l'ÉCRITURE ; l'EMBED mourait |
| `embedding_status` | 228 968 « pending » | 228 942 morts + 140 en attente |
| `diagnose_indexing` | ⛔ 716 non parsés | 5 réellement vides sur 1454 |
| `runtime_filesystem_health` | 1 issue | la nature de l'issue restait dans `data.*` |
| `qualify_indexer_truth` | présente à l'inventaire | cassée depuis deux migrations |

La règle en est sortie : `REQ-AXO-902384`. **Un dénominateur nul rend NON ARMÉ, jamais
un vert.** Et son corollaire, arraché par le cas `semantic_clones` : **un nombre porte
son échelle**.

---

## 2. Cinq diagnostics réfutés avant d'écrire une ligne

C'est le résultat le plus utile de la session, et il vaut d'être détaillé — le constat
était juste à chaque fois, la cause fausse.

**`semantic_clones` « n'applique aucun seuil ».** Le seuil existe à deux endroits, et
il mord : les distances `SIMILAR_TO` vont de 0,0000 à 0,1000 **exactement**. `<=>` de
pgvector est une DISTANCE : les « cosinus 0,06 » d'APS sont des paires à **94 % de
similarité**. Ils ont abandonné un audit de duplication légitime sur une échelle lue à
l'envers — et c'est notre en-tête `Cosine` qui les y a conduits.

**« 78 % des arêtes d'appel pendantes, les alias ne résolvent pas ».** Le chiffre est
confirmé et pire (74-93 % sur tout le parc, AXO à 84 %). Mais sur **6093 noms distincts**
pendants chez APS, **47** existent dans le projet. 99,2 % ne correspondent à rien : ce
sont des appels vers la stdlib et, pour 36,7 %, des accès de champ (`.id`) que notre
parser Elixir compte comme des appels. Un alias non résolu pointe par définition vers
quelque chose qui existe. Chantier P1 entier évité.

**`audit` « confond `.unwrap_or()` avec `.unwrap()` ».** La comparaison est exacte
(`eq_ignore_ascii_case("unwrap")`), aucun symbole `unwrap_or` n'existe dans l'index, et
`scheduling.rs:147` contient un **vrai** `.min_by_key(…).unwrap()` lu en source. L'outil
a raison ; ce qu'il ignore, c'est que ce `.unwrap()` est gardé par le `while` qui
l'englobe. Manque réel, mais distinct.

**« Le dispatch dynamique n'est pas géré ».** Deux ponts existent déjà et sont mieux
faits que ce que j'aurais écrit : pont structurel par `IMPLEMENTS`, plus un pont par nom
qui rattrape les appels atterrissant sur un nœud fantôme. Les quatre maillons vérifiés
un par un sur `ist.edge` : tous verts. Le maillon manquant est **en amont**.

**Mon propre diagnostic du chunker.** J'ai attribué les morceaux hors fenêtre au repli
sur délai et livré `bb4119b3`. Purge des 68 119 morceaux, réindex complet avec le binaire
corrigé : **les morceaux réapparaissent identiques** — 1414 jetons, 4242 caractères,
ratio 3,00. Le correctif est bon en soi, ce n'était pas la cause.

---

## 3. L'incident embed : trois causes empilées

Ce que la session 119 avait laissé comme « un » incident en contenait trois.

1. **Sursouscription VRAM** (`902373`) — l'indexeur calculait son budget sur le TOTAL de
   la carte comme s'il était seul, alors que le brain en tient 1,5 Gio.
2. **Taille de lot figée** (`902387`) — `B2_BATCH_SIZE_DEFAULT = 64` ignorait le budget.
   ORT réclamait 1,44 Gio d'arène par lot, échouait, et **tout** basculait sur CPU :
   débit divisé par 100, sans qu'aucun signal ne rougisse.
   *Le discriminant* : les petits lots du flux frais s'embeddaient sur GPU pendant que
   les lots de 64 échouaient tous, **sur la même arène**. Donc la demande par lot, pas
   le budget.
3. **Arène monotone** — l'arène BFC d'ORT ne rend jamais sa mémoire. Mesuré à 19:27,
   lots déjà à 8 : `Available memory of 19968 is smaller than requested bytes of 1572864`.
   19,5 Ko libres. Passé ce point, aucune taille de lot n'aide.

Correctif : retaillage adaptatif **plus** recyclage de session, borné à un recyclage par
lot (un appareil vraiment plein doit céder au CPU, pas boucler sur des reconstructions —
c'est un test qui a attrapé ça, le jeton était consommé par branche au lieu d'être partagé).

**Mesuré en live après promote** : `ratio 0,00 · cap 4 · 306 retaillages · 19 recyclages ·
0 bascule CPU`. Le geste manuel que je répétais toutes les 30 minutes est automatique.

---

## 4. Le piège destructif

APS a vérifié **une par une** les 22 preuves que `soll_verify_requirements` déclarait
cassées : **21 étaient valides**. Le remède que l'outil suggère lui-même,
`soll_remove_evidence(broken_only=true)`, les aurait supprimées — dont 3 attachements de
commit.

Chez nous, à l'échelle : **1173 lignes marquées `broken`**. Après typage par la FORME du
ref : **195 réelles**, zéro suppression.

La méthode qui a compté — et qui est devenue une pratique : **regarder les survivants
après chaque passe**. 1173 → 609 (hashes, ids SOLL) → 350 (`git:`, `HEAD`, notes) → 195
(refs à schéma, étiquettes). Trois fois j'aurais pu déclarer terminé.

---

## 5. Cinq promotes perdus, et ce que ça a établi

| # | Cause |
|---|---|
| 1 | 127 lignes non commitées trouvées avant build |
| 2 | 3 fichiers écrits **pendant** la compilation |
| 3 | reprise du travail sur décision opérateur — **message envoyé à 23:58, build lancé à 23:58:21** |
| 4 | worktree : `.axon/` gitignoré, donc ni artefacts ORT ni état de release ; le repli a **arrêté Axon** |
| 5 | commit en cours au moment de la prise de main |

Le 3 est le plus instructif : l'écrivain avait prévenu **à l'avance** et les deux
messages se sont croisés à 30 secondes près. La discipline était présente des deux côtés,
à chaque fois.

Deux modes de défaillance, dont un que je n'avais pas vu venir :
- **collision** — la vérification est juste quand elle est faite, périmée quand elle sert ;
- **interblocage poli** — chacun attend l'autre, rien ne rougit, le système ne progresse plus.

`REQ-AXO-902391` (P0). Le critère qui compte le plus n'est pas le worktree : *un cutover
qui se replie laisse le service debout, ou dit explicitement qu'il ne peut pas et
pourquoi.* C'est celui-là qui a coûté une interruption d'Axon.

---

## 6. Ce qui reste ouvert

**La question à trancher en premier** — 3 652 morceaux étiquetés > 512 jetons, `pending`,
non encore servis (tri croissant, ils passent en dernier).

- **(a)** ils dépassent vraiment → aligner le plafond du docstring. `cap_symbol_docstring`
  calcule avec `SAFE_CHARS_PER_TOKEN`=4, l'étiquetage avec `FALLBACK_CHARS_PER_TOKEN`=3 :
  **33 % d'écart par construction**. C'est le piège des deux constantes que `902340` a
  corrigé pour le CORPS et pas pour le docstring, quinze lignes plus haut dans le même
  fichier.
- **(b)** l'étiquette ment → corriger le compteur. Un morceau analysé est à **79 %
  d'espaces d'alignement** (4243 caractères, 145 mots) : `chars/3` annonce 1414 jetons là
  où WordPiece en compterait ~300.

Signal contradictoire à ne pas ignorer : parmi 466 330 morceaux `embedded`, le maximum
est **512 pile**. Aucun n'a jamais dépassé. Mais les échecs passés sont confondus avec
l'incident VRAM — d'où le besoin d'attendre le drain avec un embedder sain.

`select embed_status, count(*) from ist.chunk where token_count>512 group by 1`

**`REQ-AXO-902370`** — établir si `stage_a2.rs::a2_transform` est atteint depuis une
racine. Ne pas « ajouter le support du dispatch » : ce serait réécrire du code correct.

---

## Chiffres

| | |
|---|---|
| Couverture de l'index | 61,06 % → ~89 %, redescendue à ~70 % (68 119 morceaux reconstruits en attente) |
| `failed` | 145 942 → 41 473 |
| REQ livrés | 15, tous avec preuves attachées |
| Commits | `80e51cbb` `9e6af3c2` `ffdd07cf` `7b362b51` `4201b14a` `3ac88199` `a0361159` `26d85246` `fcdc4fe7` `4e766e56` `eff45d13` `874197e1` `bb4119b3` |
| Promotes | 2 réussis (`874197e1` par AXO, `bb4119b3` par VPC), 5 perdus |
