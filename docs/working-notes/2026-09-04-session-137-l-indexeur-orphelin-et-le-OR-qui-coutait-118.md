# Session 137 (2026-09-04) — l'indexeur orphelin, et le `OR` qui coûtait ×118

> Note de travail, append-only. **La SOLL fait foi** : l'état vivant est dans `CPT-AXO-052`.
> Cette note garde le *raisonnement*, pas l'état.

## La question de départ

« Pourquoi on ne maîtrise pas cet indexeur ? » — puis « analyse 5W, et réparation définitive ».
La réponse tient en une phrase : **deux pannes indépendantes se sont superposées**, et aucune
des deux n'était visible dans nos sondes.

## 5W

| W | réponse |
|---|---|
| **What** | Le superviseur live relance `axon-indexer` toutes les 30 s — **2 565 fois en 21 h** — chaque relance mourant en ~40 ms sur `Runtime writer ownership enforcement refused startup`. En parallèle, un indexeur **orphelin** (pid 650712) tient le verrou IST et échoue sa réconciliation A3 depuis 101 lots. |
| **When** | **2026-09-04 00:23:18.262**, ligne 159 de `/tmp/process-compose-dstadel.log`. |
| **Where** | `process-compose.live.yaml:183-217` · `runtime_boot.rs:823` · `bulk_writer.rs:963-1005`. |
| **Who** | `process-compose` 1.94.0 : le timer `availability.backoff_seconds` et la résolution de `depends_on: postgres-check` sont **deux chemins de démarrage non sérialisés**. |
| **Why** | Deux `Started` sans `Exited` entre eux → le superviseur écrase sa référence de processus → le premier devient orphelin, **vivant**, propriétaire du verrou → toute relance est refusée à jamais. |

Preuve : `awk` sur tout le journal rend **exactement deux** `Started` non suivis d'un `Exited`,
lignes 21 (00:00:42.869) et 159 (00:23:18.262). Pas une hypothèse — un comptage.

## Les trois raisons pour lesquelles nous ne le maîtrisions pas

1. **Aucun journal en live.** `process-compose.dev.yaml:93` porte `log_location` depuis
   `REQ-AXO-901893` ; `live.yaml` n'en avait **aucun**. Le superviseur ne garde qu'un anneau de
   500 lignes. 2 565 morts, **zéro ligne de cause** survivante. C'est la panne d'observabilité
   qui a rendu les deux autres invisibles.
2. **Aucun plafond de redémarrage, délibérément** (`REQ-AXO-902576`). Le raisonnement était juste
   pour un échec *transitoire*, et faux pour un *refus permanent*. Le runtime rend le **même
   code 1** pour les deux classes : le superviseur ne peut pas les distinguer.
3. **La sonde de vie ne distingue pas « un indexeur vit » de « l'indexeur supervisé vit ».**
   `indexer_alive` est resté vert 21 h — grâce au heartbeat de l'orphelin — pendant que
   `indexer_process_stable` était rouge.

## ⭐ La mesure qui a tout tranché : le `OR` coûte ×118

`EXPLAIN (ANALYZE, TIMING OFF)` sur la base live, projet **FSF** (23 814 chunks × 1 654 artefacts),
`statement_timeout` de production = **30 s** :

| forme de la requête A3 | temps mesuré |
|---|---|
| une requête, deux sélecteurs joints par **`OR`**, 5 artefacts neufs | **43 410 ms** ⛔ |
| chemins changés seuls (20 chemins réels) | 20 ms |
| artefacts neufs seuls (5) | 347 ms |
| artefacts neufs seuls (200) | 12 767 ms |

Le plan le dit : `Materialize … loops=23814`, soit **39,4 millions** d'appels de `position()`.
Le prédicat est une recherche de sous-chaîne qu'**aucun index ne peut servir** ; le nombre de
lignes de la jointure *est* le coût. Avec le `OR`, PostgreSQL ne peut plus restreindre par l'un
**ou** par l'autre : il élargit le scan à tout le projet.

**Cinq artefacts neufs suffisent à dépasser le délai.** C'est ainsi que `REQ-AXO-902603`, livré
la veille, a mis A3 par terre sans que personne ne le voie.

Trois faits d'appui qui ferment les fausses pistes :

- **AXO ne porte aucun `data_artifact`** — la panne ne pouvait jamais venir de notre propre index.
  Seuls FSF (1 654) et OPV (92) en portent. Diagnostiquer sur AXO ne montrait rien.
- `char_length(artifact.name) >= 4` ne filtre **rien** sur FSF : le nom le plus court fait 13
  caractères. Garde-fou nominal, jamais mordant.
- Le planificateur estime `rows=4` là où il y en a **1 654** (erreur ×400) — d'où la boucle
  imbriquée. Un `ANALYZE ist.symbol` améliorerait le plan ; il ne **borne** rien. Relever
  `statement_timeout` non plus : le produit cartésien serait repoussé, pas supprimé.

## La borne doit tenir des **deux** côtés, pas d'un seul

Casser le `OR` en deux requêtes ne suffit pas : il reste deux dimensions qui grandissent, et
en borner une seule laisse la panne revenir par l'autre porte.

| dimension | ce qui la fait grandir | borne posée |
|---|---|---|
| artefacts neufs | un scan qui découvre 1 654 JSON d'un coup | `A3_NEW_ARTIFACT_SLICE = 100` |
| chemins changés | un rescan, un gros commit, un réindex ciblé par chemin | `A3_CHANGED_PATH_SLICE = 200` |

Les chemins *étaient* déjà bornés en amont — `AXON_A3_BATCH_SIZE`, 32 par défaut
(`runtime_config.rs:50`), et le worker A3 vide son tampon dès qu'il atteint ce seuil
(`stage_a3.rs:156`). Mais c'est une **variable d'environnement**. La relever pour accélérer un
rescan ressusciterait le produit cartésien du côté des chemins, sans le moindre signal — la même
panne par l'autre porte. *Une requête qui doit tenir sous un délai porte sa propre borne.*

Calibrage, lu dans la table ci-dessus : 20 chemins coûtent 20 ms, donc 200 chemins ≈ 200 ms ;
200 artefacts coûtent 12 767 ms, donc 100 ≈ 6,4 s. Les deux restent d'un ordre de grandeur sous
les 30 s, sur le projet le plus chargé du parc.

L'invariant est épinglé par un test qui refuse **tout** tableau non borné, pas seulement celui
qu'on venait de réparer : `aucune_requete_a3_ne_recoit_un_tableau_non_borne`.

## Le design réfuté avant d'être écrit

Première idée pour casser la boucle de redémarrage : faire sortir l'indexeur refusé avec
**`exit 0`**. Réfutée avant codage. Avec `restart: on_failure`, `process-compose` marque alors le
processus **`Completed`** et ne le relance **plus jamais** pour la vie du superviseur ; le service
disparaît en silence, et `indexer_alive` resterait vert sur le heartbeat de l'orphelin.
**Strictement pire que la boucle.** Et `exit 0` est un mensonge : le processus n'a pas travaillé.

Design corrigé consigné dans `REQ-AXO-902614` (takeover quand le propriétaire est un frère vivant
du même superviseur, avec la même identité runtime — vérifié sur `/proc/650712/environ`).

## Ce que la session dit de notre méthode

**Trois lectures fausses, toutes les miennes, toutes du même genre : lire un code de retour au
lieu de lire la sortie.**

`rc=101` de cargo a été lu trois fois comme un test rouge. C'était
`error: could not find Cargo.toml in /home/dstadel/projects/axon` — **il n'y a pas de manifeste à
la racine** de ce dépôt. Exactement la leçon de la session 136, re-vécue. Le verdict est la ligne
`test result:` ; si elle est absente, il n'y a **pas eu** de test.

Second défaut de méthode, plus grave parce qu'il touchait la preuve : les cinq tests de R4 ont été
écrits **après** l'implémentation. Ils n'avaient donc jamais prouvé qu'ils savaient échouer.
Rattrapé par un **contrôle-mutant en deux passes** :

| mutant | tests rougis |
|---|---|
| retour au `OR` mono-requête | 2 / 5 |
| Q2 toujours émise, jamais tranchée, + un `$4` en trop | 4 / 5 |

Union = **5/5**. Une seule mutation n'aurait rien prouvé sur trois d'entre eux.

## Corruption de la mémoire gouvernée — cause enfin établie

`REQ-AXO-902556` mesurait 475 pratiques corrompues sur 1 912, dont 351 sans preuve. La cause a été
**reproduite en direct** : sur neuf `practice_put`, huit ont stocké dans `practice` la chaîne
littérale `</practice><parameter name="dense">…`, avec `dense` à zéro. Quand `evidence` suivait
`dense` dans l'appel, elle était perdue aussi — ce qui explique exactement la corrélation
« 78 % des corrompues sans preuve ». Un seul appel sur neuf a sérialisé correctement : la
corruption est **intermittente** et vient de l'émission, pas du serveur.

**Contournement sûr, appliqué** : ne pas passer `dense` du tout, écrire la forme dense directement
dans `practice`. Les huit corrompues ont été retirées par `practice_retire` avec `superseded_by`.

## Ce qui a été fait sur le runtime, et dans quel ordre

La porte de build s'est fait tuer **deux fois** par pression mémoire. Le diagnostic est le même
que la panne : la machine portait `axon-brain` (5,4 Gio), Postgres, **et un indexeur orphelin de
3,8 Gio qui ne produisait rien depuis 22 heures**. Le swap était à 8/8 Gio.

Séquence appliquée, chaque geste réversible :

| # | geste | pourquoi c'est sûr |
|---|---|---|
| 1 | `process-compose process stop axon-indexer -p 8080` | arrête la boucle de redémarrage — **2 686 relances** au moment du geste. Réversible par `process start`. |
| 2 | `kill -TERM 650712` (**par PID**, `GUI-AXO-1039`) | l'orphelin échouait A3 en boucle ; il ne perdait aucun travail. Sorti proprement en moins de 10 s. |

Mémoire disponible : 23 → **28 Gio**. Rien d'autre n'a été touché : le brain (pid 473977) et le
dashboard n'ont pas bougé, **aucune promotion n'a été faite**.

Preuve directe de `REQ-AXO-902616`, lue sur `/processes` du superviseur pendant la panne :

```
axon-indexer   status="Restarting"   restarts=2686   is_ready="Ready"
```

La sonde dit **`Ready`** d'un processus que le superviseur déclare en train de redémarrer, dans
la même réponse. Ce n'est pas une course : c'est une sonde qui ne regarde pas le bon processus.

## Ouvert à la clôture

`REQ-AXO-902614` (takeover, P0) · `REQ-AXO-902616` (la vivacité doit dire QUEL indexeur vit) ·
`REQ-AXO-902618` (`status` ne doit pas dire la projection fraîche pendant que l'écrivain échoue) ·
`REQ-AXO-902619` (`axon_init_project` dépasse le budget de sortie — 103 240 caractères sur AXO,
friction chiffrée aussi par DVM) · `REQ-AXO-902620` (`build_identity: match` avec un
`AXON_BUILD_ID` périmé de 83 commits).

⚠ **Le runtime reste cassé tant que 650712 n'est pas remplacé.** Ni le journal ni la borne A3 ne
le changent : l'orphelin tient le verrou et est stérile. Il faut le tuer **par PID**
(`GUI-AXO-1039`, jamais `pkill`) après promotion, en laissant les **120 s** de teardown CUDA
(`REQ-AXO-902263`) — un SIGKILL en plein teardown a déjà laissé un worker en état D.
