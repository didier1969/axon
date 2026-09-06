# Session 142 — la porte verte, et le mutant qui n'avait pas muté

**2026-09-06** · HEAD `fc68d833` · porte `GUI-AXO-1034` **VERTE** (2 227 / 0, cinq étapes `rc=0`)

## Ce que la session a fait

Fermé les **8 tests rouges** qui interdisaient le promote depuis vingt-deux commits.
Un seul commit, `fc68d833`, 6 fichiers, +377 / −46.

Quatre causes distinctes. **Aucune fragilité de test** — c'est le point le plus important :
la tentation, devant huit rouges d'un coup, est de chercher une cause commune et, faute de la
trouver, de conclure que la suite est fragile. Chacun des huit désignait un défaut réel, et trois
d'entre eux étaient des défauts de **production**, pas de test.

| # | Cause | Tests | Nœud |
|---|---|---|---|
| 1 | `token_budget` pesait l'**enveloppe** au lieu du **contenu** | 3 | `REQ-AXO-902596` |
| 2 | L'ordre de coupe était une table **statique** | (inclus au 1) | `REQ-AXO-902596` |
| 3 | `sql` avait perdu son miroir `rendered_text` | 1 | `REQ-AXO-902560` |
| 4 | `conception_view` ne **nommait** jamais ses éléments | 1 | `REQ-AXO-902409` |
| + | 3 assertions périmées par une remédiation volontaire | 3 | `REQ-AXO-902624` |

## La preuve qui était posée sur la table depuis le début

Le dump du test rendait, côte à côte, dans la même réponse :

```
estimated_tokens: 399    rendered_tokens: 788    requested_budget: 900
```

sur un paquet dont la coupe n'avait **rien** retiré. Deux nombres qui devraient être égaux et qui ne
le sont pas : c'est la signature complète du défaut, lisible sans rien exécuter. Ils étaient
calculés par deux définitions distinctes du mot « jetons » — l'une pesant le contenu utile, l'autre
la sérialisation entière du paquet, diagnostics compris.

Sur une réponse ordinaire, cette machinerie pèse ~400 jetons, soit **88 % d'un budget de 900**.
L'appelant payait ce qu'il n'avait pas demandé et ne pouvait pas refuser, puis se voyait retirer la
charge utile pour tenir un budget déjà consommé par du méta.

J'avais lu ces deux nombres sans m'y arrêter, et commencé à empiler trois pistes correctives avant
d'avoir identifié la grandeur. La grandeur, une fois nommée, a réglé **7 échecs sur 8**.

## Le second défaut, découvert seulement APRÈS le premier correctif

Le 8ᵉ test résistait. Le message d'assertion ne disait que `structural_neighbors = []` — une bande
vide, sans dire **comment** elle l'était devenue. Enrichi pour distinguer les deux façons (le graphe
n'a rien trouvé / la coupe a tout retiré), il a livré la cause en un run :

```
graph_neighbors_selected = Some(5)
omitted_bands = [structural_neighbors: 5 items / 258 jetons,
                 supporting_code_context: 7 items / 889 jetons]
estimated_tokens: 1747   rendered_tokens: 602   requested_budget: 1200
```

Le graphe **avait** trouvé les voisins. La coupe les a pris en premier pour rien : retirer
`supporting_code_context` seul suffisait (1 747 − 889 = 858 ≤ 1 200). La table statique classait
« périphérique » ce qui, sur une route `impact`, **est** la réponse.

« Périphérique d'abord » reste le bon principe. Le figer dans une constante est le bug : la
centralité d'une donnée dépend de la **question**, pas du nom du champ.

## Le mutant qui n'avait pas muté

Le contrôle-mutant (pratique `2139`) portait sur la seule garde neuve sans rouge authentique
derrière elle. Le premier run a rendu :

```
state: succeeded   exit_code: 0   durée: 97 s   pic: 5,6 Go
```

Tous les signaux d'un vrai run — et cargo **avait** réellement tourné. Sur du code **sain** :
l'injection `awk` avait échoué en silence. Lu naïvement, ce résultat disait « la garde ne détecte
pas le mutant ». En réalité aucun mutant n'avait jamais existé.

La cause : passer la regex d'ancrage par `awk -v`. awk déshabille d'abord la chaîne, `\(` devient
`(`, et la regex dynamique cesse de matcher les parenthèses littérales. Un avertissement le signale,
noyé dans la sortie. Le script ne gardait que la **restauration** — le côté où rien n'avait échoué.

Correctif : encadrer la mutation des **deux** côtés par un `sha256`. Avant, injecter, **exiger que le
hash ait changé** (code de sortie distinct sinon), jouer, restaurer, exiger le hash d'origine. Le run
suivant a rendu `MUTANT_ECHEC_ANCRE` en **4 secondes** au lieu de 97 secondes de mensonge.

Second piège du même script : injecter un `return` nu en tête de fonction rend la suite du corps
unreachable ; un crate qui refuse les warnings échoue alors à **compiler**, ce qui produit un code de
sortie non nul sans qu'aucun test n'ait tourné — et ressemble exactement à une garde rouge. Injecter
`if true { return … }`.

Contrôle réussi ensuite : `la_MACHINERIE_de_diagnostic_n_est_PAS_facturee_au_budget` rouge, **et elle
seule** — `test result: FAILED. 8 passed; 1 failed`.

## Le nombre attendu, et pourquoi il compte plus que « 0 failed »

Base : 2 216 passed / 8 failed = 2 224 tests. Plus 3 tests neufs = **2 227 attendus**.
Un log qui aurait dit `2224 passed; 0 failed` **ne serait pas vert** : il dirait que les trois tests
neufs ne sont pas enregistrés. C'est la forme exacte que décrit la pratique `2216` — un vert local
compatible avec un rouge global.

Contrôle croisé gratuit : le run mutant, sur le même arbre, annonçait `2226 filtered out` pour 9
tests joués, soit 2 235 au total. La porte a rendu `2227 passed; 0 failed; 8 ignored` — les deux
arithmétiques concordent.

## Ce qui reste

- **Le promote** : 23 commits en attente (`8ade669a..fc68d833`), plus bloqué que par l'arbitrage
  opérateur (coupure mesurée ~104 s, `REQ-AXO-902256` en brèche pour la 3ᵉ fois).
- **`ist_writer` degraded** : FK `indexedfile_project_code_fkey`, `project_code=(MRG)` absent de
  `project`. Deux hypothèses de freinage coexistent pour `REQ-AXO-902402`, aucune tranchée.

## Pratiques déposées (scope `*`)

`2231` la grandeur d'une borne · `2232` l'ordre de sacrifice statique · `2233` le script mutant.

État vivant : `CPT-AXO-052`, dernière section. La SOLL fait foi.
