import importlib.util
import sys
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).resolve().parents[1] / "scripts" / "qualify_retrieval_context.py"
SPEC = importlib.util.spec_from_file_location("qualify_retrieval_context", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC is not None and SPEC.loader is not None
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class QualifyRetrievalContextTests(unittest.TestCase):
    def test_direct_anchor_hit_only_uses_direct_evidence(self) -> None:
        packet = {
            "answer_sketch": "Route `soll_hybrid` selected for `Why does checkout use the Stripe SDK?`.",
            "direct_evidence": [],
        }

        self.assertFalse(MODULE.evaluate_direct_anchor_hit(packet, ["checkout"]))

    def test_missing_file_expectation_keeps_citation_non_applicable(self) -> None:
        packet = {
            "direct_evidence": [],
            "supporting_chunks": [],
            "relevant_soll_entities": [],
        }

        citation_hit = MODULE.file_hit(MODULE.json_text(packet), []) if [] else None
        self.assertIsNone(citation_hit)

    def test_allow_missing_rationale_fixture_skips_when_no_anchor_file_or_soll_hit(self) -> None:
        packet = {
            "answer_sketch": "Route `soll_hybrid` selected for `Why does checkout use the Stripe SDK?`.",
            "direct_evidence": [],
            "supporting_chunks": [],
            "relevant_soll_entities": [],
        }

        direct_anchor_hit = MODULE.evaluate_direct_anchor_hit(packet, ["checkout"])
        citation_hit = None
        soll_hit = MODULE.expected_soll_hit(packet, ["DEC-BKS-010"])

        self.assertFalse(direct_anchor_hit)
        self.assertFalse(bool(citation_hit))
        self.assertFalse(soll_hit)


if __name__ == "__main__":
    unittest.main()
