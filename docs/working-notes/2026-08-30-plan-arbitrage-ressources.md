# Plan — arbitrage des ressources GPU/RAM entre Axon et le parc

Session 132 · 2026-08-30 · accord opérateur sur le principe.
Priorités données : **indexeur en marche au plus vite**, **brain perturbé au minimum**.

## Le principe retenu

**Trois classes, un arbitre, une règle.**

| Classe | Qui | Droit |
|---|---|---|
| **Interactif** | Vox, requêtes MCP | sert d'abord, **préempte**, ne demande jamais |
| **Différable** | indexeur | prend ce qui reste, **rend sur demande**, bascule en RAM |
| **Essentiel** | brain (MCP + SOLL) | jamais tué, jamais bloqué, 0 VRAM au repos |

**Règle unique : jamais casser, seulement ralentir.** Pas de VRAM → l'indexeur travaille en
RAM. Plus lent, jamais absent. Aucune fonction ne disparaît côté client.

**L'arbitre est VPC**, pas Axon. Son courtier connaît déjà les classes, les priorités
(`interactive` / `scheduled`), un champ `--gpu-mib`, et refuse déjà un job GPU si la carte est
trop pleine. L'indexeur devient un **client de ce courtier**. On ne construit pas un système :
on branche l'indexeur sur celui qui marche.

**Préemption : option B retenue** — l'indexeur *anticipe* plutôt que Vox n'*attende*. Il prend
le GPU par tranches courtes et rend spontanément entre deux. Quand l'opérateur dicte, il est
déjà probablement en RAM. Motif : la dictée est un geste sans préavis ; un système qui fait
attendre l'humain pour une tâche de fond est un mauvais système. Fallback A (rendre sur ordre,
budget d'arrêt 120 s) si B ne suffit pas.

## Mesures qui fondent le dimensionnement

| Consommateur | RAM | VRAM |
|---|---|---|
| Brain au repos | 1,2 Gio | **0** |
| Worker de recherche du brain | 1,7 Gio | 1,9 Gio (plafonné 2,2 / 2,2, meurt à 5 min d'inactivité) |
| Indexeur | non mesuré (meurt trop tôt) | réserve 4,1 Gio, plafond adaptatif ~4,0 |
| Vox | 162 Mio | **478 Mio en permanence** + pointe à la dictée |
| Carte | — | 8,0 Gio |

⚠️ **Correction de `DEC-AXO-901672`.** Son point 5 dimensionne la réserve sur « le consommateur
intermittent ne détient AUCUNE VRAM au repos ». Mesuré le 2026-08-30 : `voxtype-vulkan` tient
**478 Mio depuis 33 h**, push-to-talk au repos. Plancher permanent + pointe, pas zéro + pointe.

La contention est **épisodique, pas structurelle** : 7,3 Gio libres au moment de la mesure,
indexeur mort.

---

## Tranche 0 — remettre l'indexeur en marche (IMMÉDIAT, sans toucher au brain)

Les deux causes de mort sont établies (`REQ-AXO-902576`) : crash constant à l'init CUDA, et
course DDL au bootstrap. Le mode CPU évite la première ; le brain déjà démarré (DDL appliqué)
évite la seconde.

1. Relancer le rôle indexeur seul, qui hérite de `AXON_EMBEDDING_PROVIDER=cpu` du superviseur.
2. Vérifier qu'il tient ≥ 10 min et que la fraîcheur de l'index revient.
3. **Rendre le réglage durable** dans `process-compose.live.yaml` (bloc indexeur), pour qu'un
   redémarrage ne le perde pas — appliqué au prochain redémarrage complet, pas maintenant.

C'est déjà la règle « jamais casser, seulement ralentir » : le graphe est indexé, la
vectorisation attend le correctif CUDA.

## Tranche 1 — retirer le quota de 3 redémarrages

`process-compose.live.yaml`, bloc indexeur : `max_restarts: 3`. Absurde ici — l'indexeur meurt
d'un manque de ressource et on lui interdit de revenir quand la ressource revient.

Remplacer par : redémarrage sans plafond, recul progressif (10 s → 30 s → 2 min, plafonné).
Le brain a déjà ce régime.

## Tranche 2 — « pas à jour depuis X » au lieu de « dégradé »

L'horodatage existe déjà (`truth_cockpit.staleness.last_publish_ts`). Un adjectif ne se compare
pas ; un écart de temps, si — le lecteur décide lui-même si quatre heures le gênent.

Remplacer `truth_status: degraded` par un écart explicite dans les surfaces qui le rendent :
`status`, `project_status`, `axon_init_project`, dashboard. Garder un booléen machine à côté
pour les gardes automatiques.

## Tranche 3 — le brain ne peut plus être tué par sa propre sonde

Déjà identifié (`REQ-AXO-902563`) : les 270 s actuelles ne sont qu'un desserrage. Le correctif
de fond est de servir `/livez` **hors** du chemin de réchauffage, comme `05fa97af` l'a fait pour
l'indexeur. Sans lui, un parc plus gros redéplacera le problème.

## Tranche 4 — le bail GPU révocable, arbitré par VPC

L'existant est un **verrou d'exclusion** entre instances Axon (`GpuVectorLease` : un fichier +
une identité, `vector_control.rs:307`), pas un arbitre : il ne connaît ni Vox, ni les priorités,
ni la préemption.

À construire, côté VPC :
1. L'indexeur **demande** une tranche GPU au courtier (classe `scheduled`, `--gpu-mib`).
2. Le courtier accorde si la carte le permet, **réserve incluse** pour Vox et le brain.
3. Le bail est **court et renouvelable** (option B) : l'indexeur rend spontanément entre deux
   tranches.
4. Un consommateur `interactive` qui arrive **révoque** le bail ; l'indexeur bascule en RAM.
5. Sans bail : l'indexeur travaille en RAM. Jamais d'arrêt.

Prérequis : le correctif du crash init CUDA (`REQ-AXO-902576`) — sans lui, l'indexeur ne peut
de toute façon pas prendre le GPU.

---

## Ordre d'exécution

| # | Tranche | Pourquoi ce rang |
|---|---|---|
| 0 | indexeur en marche (CPU) | priorité opérateur, zéro perturbation du brain |
| 1 | quota de redémarrage | empêche l'indexeur de revenir seul — bloque tout le reste |
| 2 | vocabulaire de fraîcheur | quasi gratuit, améliore chaque lecture de statut |
| 3 | brain non bloquant | supprime la cause d'arrêt inexpliqué du service essentiel |
| 4 | bail GPU chez VPC | le gros morceau, dépend du correctif CUDA |

## Ce qui reste à trancher

- La granularité de la tranche GPU en option B (durée d'un bail, cadence de renouvellement) —
  à calibrer sur une mesure, pas à choisir a priori.
- Ce que VPC expose comme surface de révocation (signal, fichier, appel) — décision côté VPC.
