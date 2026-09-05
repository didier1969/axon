#!/usr/bin/env python3
"""Estime la durée jusqu'au cutover à partir du journal des tentatives — REQ-AXO-902543.

Lit les `*.jsonl` de `<dir>` et imprime UNE ligne destinée à l'opérateur.

Pourquoi ce fichier existe séparément, et testé
----------------------------------------------
C'était un heredoc en ligne dans `promote_live_safe.sh`, et il retenait une
tentative comme « réussie » sur ce prédicat :

    any(r["event"] == "lease_released" and r["status"] == "completed")

Le bail est relâché `completed` dès que le SCRIPT se termine proprement — y
compris quand le cutover a échoué et que l'auto-rollback a fait son travail. Le
prédicat mesurait donc « le script s'est arrêté sans planter », et la phrase
imprimée disait « tentative(s) réussie(s) ». Mesuré sur les 76 tentatives du
journal au 2026-09-05 : **22 comptées, 1 à tort** (20260827T035308Z) — rare, mais
c'est le chiffre que l'opérateur lit AVANT de décider de promouvoir, et la classe
de défaut est celle que toute cette tranche corrige : une étiquette qui affirme
plus que la mesure.

Le prédicat retenu est l'ÉVÉNEMENT qui prouve le franchissement :
`step_completed` sur la phase `cutover_finalize` avec `status == "passed"`.

C'est la deuxième fois qu'un instrument du promote se révèle faux dans la
direction flatteuse (`mcp_outage_report.py` porte l'autre histoire). D'où le même
remède : un fichier, une fonction pure, des cas de test dont un MUTANT.
"""

import json
import pathlib
import statistics
import sys

# Phases dont le DÉBUT marque « on entre dans le cutover » — le point de mesure.
PHASES_DEBUT_CUTOVER = {"cutover", "cutover_prepare"}


def lignes(path):
    """Lit un `.jsonl` en ignorant les lignes illisibles (un journal peut être
    tronqué par un SIGKILL : c'est précisément le cas qu'il doit survivre)."""
    out = []
    try:
        brut = path.read_text(encoding="utf-8")
    except OSError:
        return out
    for line in brut.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            out.append(json.loads(line))
        except ValueError:
            continue
    return out


def a_franchi_le_cutover(rows):
    """VRAI seulement si le cutover a été finalisé. `lease_released/completed` ne
    le dit pas : le bail se relâche proprement même après un rollback."""
    return any(
        r.get("event") == "step_completed"
        and r.get("phase") == "cutover_finalize"
        and r.get("status") == "passed"
        for r in rows
    )


def duree_jusqu_au_cutover(rows):
    """Secondes entre le premier événement et l'entrée dans le cutover, ou None."""
    if not rows:
        return None
    debut = rows[0].get("monotonic_ms")
    cut = next(
        (
            r.get("monotonic_ms")
            for r in rows
            if r.get("event") == "step_started" and r.get("phase") in PHASES_DEBUT_CUTOVER
        ),
        None,
    )
    if isinstance(debut, int) and isinstance(cut, int) and cut >= debut:
        return (cut - debut) // 1000
    return None


def estimate(journaux, fenetre=20):
    """PURE. `journaux` = liste de listes d'événements, du plus ancien au plus récent.

    Rend la phrase à imprimer. Ne compte QUE les tentatives ayant franchi le
    cutover — une tentative annulée avant lui n'a pas de durée « jusqu'au
    cutover » à offrir, et une tentative qui a échoué AU cutover n'est pas une
    réussite."""
    durees = []
    for rows in journaux[-fenetre:]:
        if not a_franchi_le_cutover(rows):
            continue
        d = duree_jusqu_au_cutover(rows)
        if d is not None:
            durees.append(d)
    if not durees:
        return "historique insuffisant; aucune durée promise"
    return (
        "médiane historique jusqu'au cutover=%ds sur %d tentative(s) ayant FRANCHI le cutover"
        % (int(statistics.median(durees)), len(durees))
    )


def main():
    if len(sys.argv) < 2:
        print("historique indisponible")
        return 0
    racine = pathlib.Path(sys.argv[1])
    try:
        fichiers = sorted(racine.glob("*.jsonl"))
    except OSError:
        print("historique indisponible")
        return 0
    print(estimate([lignes(f) for f in fichiers]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
