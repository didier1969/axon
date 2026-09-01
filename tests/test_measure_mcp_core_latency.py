# RÉUTILISE : scripts/measure_mcp_core_latency.summarise_samples pour la fonction sous
# test ; le chargement par `importlib.util.spec_from_file_location` + `unittest` est la
# forme des tests voisins (tests/test_mcp_probe_common.py, tests/test_mcp_validate.py) —
# vérifié via `axon query "measure_mcp_core_latency mesure la latence de chaque outil une
# seule fois"` (aucun symbole couvrant le résumé d'échantillons).
"""REQ-AXO-902589 — la porte ne doit plus se faire piloter par un seul échantillon.

Tests sur la fonction PURE : aucun runtime, aucun réseau. Le défaut qu'ils gardent
a coûté trois promotes le 2026-09-01, chacun avec 22 s de coupure MCP contiguë, sur
trois outils différents — et aucun n'était imputable au binaire candidat.
"""

import importlib.util
import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parents[1] / "scripts"
# Le module sous test importe `mcp_probe_common` en tête ; sans ce chemin, le
# chargement échoue AVANT d'atteindre la fonction pure que l'on veut éprouver.
sys.path.insert(0, str(SCRIPTS_DIR))

MODULE_PATH = SCRIPTS_DIR / "measure_mcp_core_latency.py"
SPEC = importlib.util.spec_from_file_location("measure_mcp_core_latency", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC is not None and SPEC.loader is not None
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)

summarise_samples = MODULE.summarise_samples


class SummariseSamplesTest(unittest.TestCase):
    def test_un_aberrant_isole_ne_pilote_plus_le_verdict(self):
        """LE cas qui a fait échouer trois promotes.

        Profil réel mesuré : `status` à 10 ms, sauf un appel sur soixante à
        1 570 ms. La médiane doit rester la valeur normale — sinon la porte
        compare 1 570 ms à un baseline de 9,2 ms et déclare une régression de
        170× qui n'existe pas.
        """
        resume = summarise_samples([10.0, 1570.0, 10.2])
        self.assertEqual(resume["latency_ms"], 10.2, resume)
        # …mais l'aberrant n'est PAS effacé : une porte aveugle à sa propre queue
        # serait le défaut inverse, et c'est le défaut de fond de REQ-AXO-902589.
        self.assertEqual(resume["latency_max_ms"], 1570.0)
        self.assertEqual(resume["samples_over_1500ms"], 1)
        self.assertEqual(resume["latency_samples_ms"], [10.0, 1570.0, 10.2])

    def test_une_vraie_degradation_deplace_bien_la_mediane(self):
        """Le contrôle POSITIF, sans lequel le précédent ne prouve rien.

        Si la médiane absorbait tout, la porte cesserait de garder quoi que ce
        soit. Trois échantillons lents ⇒ verdict lent.
        """
        resume = summarise_samples([1600.0, 1700.0, 1800.0])
        self.assertEqual(resume["latency_ms"], 1700.0)
        self.assertEqual(resume["samples_over_1500ms"], 3)

    def test_la_mediane_n_est_pas_le_minimum(self):
        """Un `min` aurait rendu la porte aveugle : 2 lents sur 3 restent lents."""
        resume = summarise_samples([10.0, 1600.0, 1700.0])
        self.assertEqual(
            resume["latency_ms"], 1600.0, "un `min` aurait rendu 10 ms et tout masqué"
        )
        self.assertEqual(resume["latency_min_ms"], 10.0)

    def test_un_seul_echantillon_reste_licite(self):
        """`--samples 1` rend l'ancien comportement, sans division ni exception."""
        resume = summarise_samples([42.0])
        self.assertEqual(resume["latency_ms"], 42.0)
        self.assertEqual(resume["latency_min_ms"], 42.0)
        self.assertEqual(resume["latency_max_ms"], 42.0)
        self.assertEqual(resume["samples_over_1500ms"], 0)

    def test_le_nombre_pair_d_echantillons_ne_casse_rien(self):
        """`statistics.median` interpole sur un nombre pair — voulu et borné."""
        self.assertEqual(summarise_samples([10.0, 20.0])["latency_ms"], 15.0)

    def test_la_cle_lue_par_les_consommateurs_existe_toujours(self):
        """`measure_mcp_suite.py` et `compare_mcp_runs.py` lisent `latency_ms`.

        Renommer cette clé les aurait cassés en SILENCE : la comparaison serait
        tombée à zéro outil apparié, et une porte qui ne compare rien PASSE.
        """
        resume = summarise_samples([1.0, 2.0, 3.0])
        self.assertIn("latency_ms", resume)
        self.assertIsInstance(resume["latency_ms"], float)


SUITE_PATH = SCRIPTS_DIR / "measure_mcp_suite.py"
SUITE_SPEC = importlib.util.spec_from_file_location("measure_mcp_suite", SUITE_PATH)
SUITE = importlib.util.module_from_spec(SUITE_SPEC)
assert SUITE_SPEC is not None and SUITE_SPEC.loader is not None
sys.modules[SUITE_SPEC.name] = SUITE
SUITE_SPEC.loader.exec_module(SUITE)


class SummarizeCoreTest(unittest.TestCase):
    """REQ-AXO-902589 — la médiane ne doit pas rendre le résumé SOURD.

    Remplacer une porte trop bruyante par une porte qui ne voit plus rien serait
    le défaut inverse, et c'est le défaut de fond que ce REQ décrit.
    """

    def test_un_aberrant_isole_ne_declenche_plus_le_verdict_mais_reste_RENDU(self):
        resume = SUITE.summarize_core(
            {
                "results": [
                    {
                        "tool": "retrieve_context",
                        "ok": True,
                        "latency_ms": 6.0,
                        "latency_max_ms": 1769.3,
                        "samples_over_1500ms": 1,
                    }
                ]
            }
        )
        # Le verdict ne bronche pas : c'est le but.
        self.assertEqual(resume["slow_tools_over_1500ms"], [])
        # Mais la queue est visible : un opérateur voit ce que la machine a fait.
        self.assertEqual(len(resume["tail_over_1500ms"]), 1)
        self.assertEqual(resume["tail_over_1500ms"][0]["latency_max_ms"], 1769.3)
        self.assertEqual(resume["tail_over_1500ms"][0]["samples_over_1500ms"], 1)

    def test_une_vraie_lenteur_declenche_toujours_le_verdict(self):
        """Contrôle POSITIF : sans lui, « le verdict ne bronche pas » ne prouve rien."""
        resume = SUITE.summarize_core(
            {
                "results": [
                    {
                        "tool": "why",
                        "ok": True,
                        "latency_ms": 1700.0,
                        "latency_max_ms": 1800.0,
                        "samples_over_1500ms": 3,
                    }
                ]
            }
        )
        self.assertEqual(len(resume["slow_tools_over_1500ms"]), 1)
        self.assertEqual(len(resume["tail_over_1500ms"]), 1)

    def test_un_outil_rapide_n_apparait_dans_aucune_des_deux_listes(self):
        resume = SUITE.summarize_core(
            {
                "results": [
                    {
                        "tool": "status",
                        "ok": True,
                        "latency_ms": 3.7,
                        "latency_max_ms": 3.8,
                        "samples_over_1500ms": 0,
                    }
                ]
            }
        )
        self.assertEqual(resume["slow_tools_over_1500ms"], [])
        self.assertEqual(resume["tail_over_1500ms"], [])
        self.assertEqual(resume["ok_tools"], 1)


if __name__ == "__main__":
    unittest.main()
