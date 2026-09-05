#!/usr/bin/env python3
"""Score an MCP-availability sample file — REQ-AXO-902233.

Reads the CSV written by `_start_mcp_sampler` (`<unix_seconds>,up|down` per line) and
prints five space-separated integers on one line:

    <worst_outage_s> <total_unreachable_s> <sample_count> <span_s> <resolution_s>

Why this is a separate, tested file
-----------------------------------
It used to be an inline heredoc that counted SAMPLES and printed them with an "s" suffix:

    if parts[1] == 'down':
        run += 1; total += 1; worst = max(worst, run)
    ...
    "worst contiguous outage: ${worst}s"

The timestamps were written to column 1 and never read. During an outage each probe costs
`curl -m 2` plus `sleep 1`, so samples land ~3s apart instead of 1s — and the report
under-stated every real outage by ~3x, in the flattering direction:

    reported 10s -> 26-28s actual   |   reported 63s -> 187s actual
    reported  8s -> 24s    actual   |   reported 49s -> 150s actual

The line below it even printed "measured resolution ~3s", so the report contradicted
itself. This is the FOURTH broken instrument in REQ-AXO-902233's history (its body records
three), which is why the scorer now lives in a file with tests instead of in a heredoc.

Definition of an outage, and why it is measured this way
-------------------------------------------------------
The outage is the elapsed time between the last sample that answered and the next one that
answered — NOT the span from first to last `down`. A client hitting the endpoint one second
after the last `up` is already refused, so the window opens then. This is the pessimistic
(honest) reading; the optimistic one would hide up to one sampling interval at each edge.

A trailing run of `down` samples with no recovery is counted to the LAST sample: the file
cannot say when (or whether) service came back, and inventing a recovery time would be the
same class of lie.

`resolution_s` is the MEDIAN interval actually observed between consecutive samples. It is
reported so nobody quotes this instrument more finely than it can measure — a "< 1s" claim
from a 3s instrument is not a measurement.
"""

import sys


def score(rows):
    """Pure scorer. `rows` = iterable of (timestamp:int, state:str).

    Returns (worst_s, total_s, count, span_s, resolution_s).
    """
    rows = [(t, s) for t, s in rows]
    n = len(rows)
    if n == 0:
        return (0, 0, 0, 0, 0)

    span = rows[-1][0] - rows[0][0]

    worst = 0
    total = 0
    last_up = None       # timestamp of the most recent answering sample
    open_since = None    # start of the current outage window, or None

    for ts, state in rows:
        if state == "up":
            if open_since is not None:
                gap = ts - open_since
                worst = max(worst, gap)
                total += gap
                open_since = None
            last_up = ts
        else:
            if open_since is None:
                # The window opens at the last KNOWN-good moment. With no prior `up` the
                # file starts mid-outage and this sample is the earliest evidence we have.
                open_since = last_up if last_up is not None else ts

    if open_since is not None:
        # Never recovered within the samples. Count to the last sample and no further.
        gap = rows[-1][0] - open_since
        worst = max(worst, gap)
        total += gap

    # Median inter-sample interval — measured, not assumed. Ties break UPWARD (`len // 2`
    # on an even count picks the upper of the two middles): an instrument must never
    # advertise itself as finer than it is, so the coarser half is the honest claim.
    deltas = sorted(
        rows[i][0] - rows[i - 1][0]
        for i in range(1, n)
        if rows[i][0] >= rows[i - 1][0]
    )
    resolution = deltas[len(deltas) // 2] if deltas else 0

    return (worst, total, n, span, resolution)


def parse(path, column=1):
    """REQ-AXO-902604 — `column` choisit la DIMENSION lue.

    1 = disponibilité du BRAIN (`tools/list` répond), la seule mesurée jusqu'ici.
    3 = disponibilité COMPLÈTE (brain répond ET l'indexeur est `readyz`).

    VPC a mesuré l'écart après la promotion c5ed296b : le promote annonçait 16 s de
    coupure contiguë, le client en percevait ~45 s. Les deux chiffres étaient justes —
    ils ne mesuraient pas la même chose. Un promote qui publie le plus flatteur des
    deux n'est pas faux, il est incomplet, et l'incomplétude se lit comme un démenti.

    Une ligne trop courte pour porter la colonne demandée est IGNORÉE plutôt que
    comptée `down` : un fichier écrit par une version antérieure du sondeur doit rendre
    « non mesuré », jamais « coupure totale ».
    """
    rows = []
    with open(path, "r", encoding="utf-8") as fh:
        for line in fh:
            parts = line.strip().split(",")
            if len(parts) <= column:
                continue
            try:
                ts = int(parts[0])
            except ValueError:
                continue
            if parts[column] not in ("up", "down"):
                continue
            rows.append((ts, parts[column]))
    return rows


def main():
    if len(sys.argv) < 2:
        print("0 0 0 0 0")
        return 0
    # REQ-AXO-902604 — argument optionnel `--column N`. La forme de sortie ne change
    # PAS : l'appelant lit toujours cinq entiers, et interroge deux fois pour obtenir
    # les deux dimensions. Ajouter des colonnes à la sortie aurait cassé le
    # `read -r worst total n span res` du script de promotion.
    column = 1
    args = [a for a in sys.argv[1:]]
    if "--column" in args:
        i = args.index("--column")
        try:
            column = int(args[i + 1])
        except (IndexError, ValueError):
            print("0 0 0 0 0")
            return 0
        del args[i : i + 2]
    if not args:
        print("0 0 0 0 0")
        return 0
    try:
        rows = parse(args[0], column)
    except OSError:
        print("0 0 0 0 0")
        return 0
    print("%d %d %d %d %d" % score(rows))
    return 0


if __name__ == "__main__":
    sys.exit(main())
