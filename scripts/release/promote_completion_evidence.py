#!/usr/bin/env python3
"""REQ-AXO-902628 — la PREUVE de complétion d'une tentative de promote.

Lit un journal `attempts/<id>.jsonl` et rend, sur une ligne :

    complete:<n> steps, cutover_finalize passed
    incomplete:<raison>

Deux conditions, et AUCUNE constante à maintenir :

1. toute étape DÉMARRÉE porte son `step_completed` — une étape restée ouverte
   est une mort en plein vol ;
2. `cutover_finalize` figure parmi les étapes terminées — c'est la dernière, et
   c'est déjà le prédicat de `promote_history_estimate.py` depuis
   REQ-AXO-902543 : les deux surfaces jugent enfin sur le même fait.

POURQUOI PAS UN COMPTE EN DUR. Le nœud annonçait « sept `step_completed` ». Le
vrai nombre est QUINZE — vérifié sur cinq promotes complets, sur les 16 sites
d'appel de `run_step`, et sur 40 commits d'historique du script (10 → 11 → 13 →
15 → 16). Un verdict codé sur 7 aurait été faux dès son écriture. Et 15 le
serait pour un promote légitimement raccourci : trois journaux archivés portent
14 étapes, sans orpheline et avec le cutover franchi (`--skip-build` n'émet pas
de `run_step`). Le prédicat ci-dessus les accepte ; un compte en dur les aurait
accusés.

Il a un JUMEAU en Rust, `release_reconciler::preuve_de_completion`, parce que
l'écrivain est du shell (il ne peut pas appeler du Rust) et que la porte
`promote_status` est du Rust (elle ne doit pas forker un `python3` par appel :
`promote_live_safe.sh` la relit en boucle pendant le cutover). Les deux textes
doivent coïncider caractère pour caractère — une garde croisée les joue sur les
mêmes journaux réels et refuse la moindre divergence.

Ce fichier est autonome À DESSEIN : il est appelé par `promote_live_safe.sh` et
exercé par une garde Rust qui, elle, tourne dans la porte `GUI-AXO-1034`. Les
`tests/shell/*.sh` se lancent à la main et ne passent dans aucun runner — une
garde qui ne tourne pas est une garde muette, et c'est précisément la famille de
défauts que ce REQ ferme.
"""
import json
import sys

MARQUE_DE_COMPLETION = "cutover_finalize passed"
ETAPE_TERMINALE = "cutover_finalize"
# Rendu d'une phase absente. Le MÊME mot que le jumeau Rust
# (`release_reconciler::preuve_de_completion`) : sans lui, Python écrirait `None`
# et la garde croisée accuserait une divergence qui n'en est pas une.
PHASE_ABSENTE = "<none>"


def preuve_de_completion(lignes) -> str:
    """Le prédicat, pris sur un itérable de lignes — donc exerçable sans fichier."""
    demarrees, terminees = [], set()
    for ligne in lignes:
        ligne = ligne.strip()
        if not ligne:
            continue
        try:
            evenement = json.loads(ligne)
        except ValueError:
            # Une ligne illisible n'est pas une preuve d'incomplétude : le
            # journal est append-only + fsync, une troncature partielle reste
            # possible sur une mort brutale. On l'ignore et on juge sur le reste.
            continue
        phase = evenement.get("phase") or PHASE_ABSENTE
        evenement_nom = evenement.get("event")
        if evenement_nom == "step_started":
            demarrees.append(phase)
        elif evenement_nom == "step_completed":
            terminees.add(phase)

    orphelines = [etape for etape in demarrees if etape not in terminees]
    if orphelines:
        return (
            f"incomplete:step(s) started but never completed: "
            f"{', '.join(str(e) for e in orphelines)} ({len(terminees)} step(s) done)"
        )
    if ETAPE_TERMINALE not in terminees:
        dernier = demarrees[-1] if demarrees else PHASE_ABSENTE
        return (
            f"incomplete:{ETAPE_TERMINALE} never completed — {len(terminees)} "
            f"step(s) done, last started `{dernier}`"
        )
    return f"complete:{len(terminees)} steps, {MARQUE_DE_COMPLETION}"


def main() -> int:
    if len(sys.argv) != 2:
        print("incomplete:usage — promote_completion_evidence.py <journal.jsonl>")
        return 0
    chemin = sys.argv[1]
    try:
        with open(chemin, encoding="utf-8", errors="replace") as fichier:
            print(preuve_de_completion(fichier))
    except OSError as erreur:
        # Jamais muet, jamais vide : l'appelant doit pouvoir citer la raison.
        print(f"incomplete:journal unreadable ({chemin}) — {erreur}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
