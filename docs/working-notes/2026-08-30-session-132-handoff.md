# Session 132 — handoff (2026-08-30)

⚠️ **Ce fichier est le REPLI.** `soll_manager append_section` sur `CPT-AXO-052` a expiré
deux fois (brain lent, embedder en CPU). **À reporter dans `CPT-AXO-052` dès que le brain
répond** — c'est le canal canonique.

## Runtime réel à la clôture

| | |
|---|---|
| `main` | **`ce2f0305`**, arbre propre, **0 commit en avance** (tout poussé) |
| Brain | pid **2578570**, redémarré 18:05, **RSS 1,17 Gio** (contre 4,4), **VRAM 0 Mio** (contre 3 598) |
| Sonde vivacité | **270 s** — YAML relu, correctif `14c9cf4d` enfin appliqué. Brain → readyz **3,4 s** |
| Indexeur | **MORT**, ne tient pas. Deux causes établies, voir plus bas |
| Dashboard | Running/Ready, port 44127 |
| `soll_validate` | 1 violation **étrangère** : `REQ-AXO-902571`, autre session sur le même brain |
| ⚠️ `AXON_EMBEDDING_PROVIDER` | dernier démarrage en **`cpu`** (diagnostic). Le YAML dit `cuda` |

## ⛔ `practice_put` injoignable — fallback posé

Deux `Axon Backend is unavailable or timed out`. L'embedder est en CPU : l'embedding d'un
corps de pratique dépasse le timeout MCP. Lectures OK, écritures `practice_*` non.

Leçon écrite en fallback : `~/.claude/projects/-home-dstadel-projects-axon/memory/feedback_falsifier-une-explication-de-panne.md`
**À migrer vers `practice_put` quand le brain répond en `cuda`.**

## Livré — 8 commits, tous poussés

| Commit | Objet |
|---|---|
| `99d3dbe0` | navigateur Wallaby épinglé + préflight durci — 28 → 13 échecs |
| `adf54c14` | fixture MCP dans l'`Application` env — 13 → 6 |
| `2635b274` | features alignées sur la topologie réelle (B1 retiré) — 6 → 1 |
| `746e4a34` | `wait_until_path` promu dans `FeatureCase` |
| `e79f47a4` | `refute_has` resynchronisé — le test instable restant |
| `b02dec99` | citations Rust : `REQ-AXO-901975`, pas `901746` |
| `ce2f0305` | la page pipeline cesse d'annoncer `NOTIFY` / `demand_pull_b` |
| `04768387` | analyse macro/méso du potentiel restant |

**E2E : 0 échec sur 6 courses à `max_cases 12`, dont 3 en classe `huge`** (41,2-41,8 s).
Toujours citer le `max_cases` avec un résultat : à 4 la suite était verte alors qu'un test
était instable.

SOLL livrées : `902569` `902570` `902572` `902574` `902575`. Ouvertes : `902573` (P1),
**`902576` (P0)**.

## ⛔ L'indexeur — DEUX causes, la VRAM n'en est pas une (`REQ-AXO-902576`, P0)

**L'explication « le brain retient la VRAM » est RÉFUTÉE.** Brain à 0 Mio, 7,7 Gio libres,
mort à la même milliseconde. Ne pas rouvrir cette piste.

1. **Crash constant à l'init CUDA.** 3 démarrages sur 3 s'arrêtent au même point :
   `TensorRT EP unavailable, using CUDA EP` — puis **rien**. `exit=-1`, aucun panic, aucune
   trace noyau. Ce n'est **pas** TensorRT : manifeste `onnxruntime_cuda_system`, le repli est
   nominal.
2. **Course DDL au bootstrap** (visible seulement en `cpu`) : `Fatal Error initializing
   GraphStore: could not create unique index mailbox_message_idem_idx` (23505).
   `db/ddl/15_mailbox.sql:61` crée l'index 2 colonnes ; `20_mailbox_pubsub.sql:86` fait un
   `DROP` **inconditionnel** puis recrée la version 3 colonnes. En base chaque clé de broadcast
   existe **75 fois** : la version 2 colonnes est structurellement impossible.

## Trois surfaces qui ont menti (`MIL-AXO-054`)

- `role_exit_event` **pas rafraîchi par tentative** — affichait « 29/08 22:36 » pendant les
  relances. M'a fait rapporter « mort depuis 15 h » alors qu'il tournait à 15:00.
- Superviseur : `restarts=1` après plusieurs relances.
- Le préflight GPU **accuse TensorRT** alors que son absence est nominale.

## 📥 Courrier NON TRAITÉ — CSAT de CSC (`msg-3131e9f489082fea59b48adf`)

Lu, **pas traité**. 8/10, et un défaut qui touche **tous les tenants** :

> ajouter des `acceptance_criteria` pour réparer une violation `GUI-PRO-126` fait basculer
> `soll_verify_requirements` de `partial` à **`done`** — sur une exigence dont 3 critères sur
> 10 sont explicitement OUVERTS.

Cause : la règle `done` teste **l'EXISTENCE** des critères, jamais leur **satisfaction**.
C'est le chemin NORMAL de réparation que `126` prescrit. CSC a contourné en passant
`REQ-CSC-046` à `blocked` — changer un statut pour corriger un compteur.

Second irritant : `soll_verify_requirements project_code=CSC` rend **146 KB** pour 63
exigences et dépasse la limite du client (`details[]` porte les actions même pour les 61
`done`). Un mode `brief` réglerait le cas.

**À logger en REQ AXO dès la reprise** — reproductible en 3 appels sur `REQ-CSC-046`.

## Trois prochaines actions

1. **`REQ-AXO-902576` (P0)** — instrumenter l'init CUDA : trace avant/après `ort::Session`
   + handler `SIGSEGV`/`SIGABRT` avec backtrace. Puis verrou consultatif PG sur le bootstrap DDL.
2. **Logger le défaut CSC** (existence vs satisfaction) — il fausse le compteur `done` de tous
   les tenants, donc l'axe `intent_alignment` de `902573`.
3. **`REQ-AXO-902573` (P1)** — établir le dénominateur honnête : combien des 636 nœuds
   orphelins DOIVENT porter une preuve. Sans lui, 0,635 n'est pas interprétable.

## Bloqué sur l'opérateur

- **Trois jobs `nexus-job` orphelins inannulables** (`AXO_CHECK`, `AXO_CHK3`, un 12 G) : le
  propriétaire est le PID du shell, et le rôle opérateur exclut `nexus-sessions.slice`. Seul un
  shell hors de cette slice peut les annuler. Relevé envoyé à VPC (`msg-d8018f238dddf85d4cd11a86`).
- **Le crash CUDA demande un arbitrage** : instrumenter le natif, ou basculer durablement
  l'indexeur en `cpu` (le graphe fonctionne, la vectorisation non).

## Réserve de mesure

IST **gelée depuis le 29/08 22:36**, code-intel 856/918. SHI 0,783 ; `duplication` 0,402 ·
`intent_alignment` 0,635 · `main_sequence` 0,648 (Δ = −1e-16) · `resilience` 0,878.
Données SOLL lues en direct.

Écart de formatage **antérieur** dans 4 fichiers `src/dashboard` (`config/test.exs`,
`projects_test.exs`, `errors_test.exs`, `pipeline_live.ex`), vérifié contre `HEAD`. Non corrigé.

Analyse macro/méso : https://claude.ai/code/artifact/00e7887e-89f5-4800-8203-f34c8c2eed5f
