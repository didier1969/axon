# Suite E2E du dashboard — le navigateur n'était pas celui du préflight

`REQ-AXO-902569` · 2026-08-30 · session 132 · commit de départ `1e04476b`

## Le fait

`bash scripts/test-dashboard-e2e.sh`, soumis au courtier (`nexus-job`, classe `huge`, 8 GiB).

| | Avant | Après |
|---|---|---|
| Job | `1788094398-ce62586b` | `1788094637-662101f4` |
| Durée | 12,4 s | 68,2 s |
| Comptes | **33 tests, 28 échecs** | **33 tests, 13 échecs** |
| `invalid session id` | **28** | **0** |
| `mismatched version of Chrome` | présent | **absent** |

La durée qui sextuple est le signe le plus net : avant, chaque test mourait à la création de
session sans rien évaluer ; après, ils s'exécutent.

## La cause

`deps/wallaby/lib/wallaby/chrome.ex:223` — `find_chrome_executable/0` sonde
`["google-chrome", "chromium", "chromium-browser"]`, **`google-chrome` en premier**.
L'hôte porte `/usr/bin/google-chrome` = **Google Chrome 151.0.7922.137**, hors devenv.
Le chromedriver de devenv est **146.0.7680.80**. Majeurs différents ⇒ chromedriver refuse
d'ouvrir la session ⇒ « invalid session id » à chaque test.

`config/test.exs` déclarait `chromedriver: [headless: true]` **sans `binary:`** : rien
n'épinglait le navigateur.

## Pourquoi le préflight n'a rien vu

Il vérifiait la **présence** de `chromedriver` et `chromium`, et imprimait leurs versions —
concordantes toutes les deux (146). Un lecteur en concluait que l'appariement était bon.
Mais Wallaby ne pilotait ni l'un ni l'autre côté navigateur. Un préflight qui ne peut pas
contredire le résultat ne prouve rien.

Après correctif, il nomme le binaire réellement retenu :

```
[preflight] chromedriver=ChromeDriver 146.0.7680.80 (...)
[preflight] browser=/nix/store/b6vcxswr4zr4aqb6rywk4h9cxj2a7984-chromium-146.0.7680.80/bin/chromium -> Chromium 146.0.7680.80
[preflight] driver and browser agree on major 146
```

## Les 13 échecs qui restent — préexistants, hors périmètre

Tous sont de **vraies** assertions : 12 `Wallaby.ExpectationNotMetError` (un sélecteur CSS
attendu est absent de la page) et 1 `Wallaby.StaleReferenceError`.

| Fichier | Échecs |
|---|---|
| `features/mcp_test.exs` | 6 |
| `features/pipeline_test.exs` | 5 |
| `features/projects_test.exs` | 1 |
| `features/errors_test.exs` | 1 |
| `features/nav_test.exs` | **0 — entièrement vert** |

Imputation : les 5 fichiers features datent de mai 2026, `src/dashboard` a été modifié jusqu'au
2026-07-24 (`9b8df19a`). C'est une **dérive tests / code**. Le correctif ne pose que `binary:`
sur le pilote : il ne peut pas changer ce qu'une page rend, donc aucun de ces 13 échecs ne lui
est imputable. Une comparaison avant/après serait vide de sens — la course de référence
n'évaluait aucune assertion.

## Défaut annexe consigné

`scripts/test-dashboard-e2e.sh`, `src/dashboard/test/README.md` et quatre commentaires de
`config/test.exs` citent `REQ-AXO-901649` comme origine de la suite E2E. Ce nœud porte en
réalité « Pipeline V2 indexer drain hang », `delivered`. La référence est fausse — non corrigée
dans cette tranche (elle traverse trois fichiers).
