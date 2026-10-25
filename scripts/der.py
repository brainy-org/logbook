# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy"]
# ///
"""Diarization error rate of hypothesis RTTMs against a reference.

uv run scripts/der.py REF.rttm HYP.rttm [--collar 0.25]

Frames of 10 ms; overlapped speech is scored; speakers are mapped one to
one to maximize overlap (Hungarian). A collar around every reference
boundary is not scored (0.25 s, as VoxConverse and most papers). Prints
DER with its parts (missed speech, false alarm, confusion) and the number
of speakers in each file.
"""

import argparse

import numpy as np
from scipy.optimize import linear_sum_assignment

STEP = 0.01


def read(path):
    turns = []
    for line in open(path):
        f = line.split()
        if len(f) >= 8 and f[0] == "SPEAKER":
            start, dur = float(f[3]), float(f[4])
            turns.append((start, start + dur, f[7]))
    return turns


def matrix(turns, n, speakers):
    m = np.zeros((len(speakers), n), dtype=bool)
    index = {s: i for i, s in enumerate(speakers)}
    for a, b, s in turns:
        m[index[s], int(round(a / STEP)) : int(round(b / STEP))] = True
    return m


def main():
    p = argparse.ArgumentParser()
    p.add_argument("ref")
    p.add_argument("hyp")
    p.add_argument("--collar", type=float, default=0.25)
    a = p.parse_args()
    ref, hyp = read(a.ref), read(a.hyp)
    end = max([b for _, b, _ in ref] + [b for _, b, _ in hyp] + [0])
    n = int(round(end / STEP)) + 1
    rs = sorted({s for _, _, s in ref})
    hs = sorted({s for _, _, s in hyp})
    R, H = matrix(ref, n, rs), matrix(hyp, n, hs)
    scored = np.ones(n, dtype=bool)
    c = int(round(a.collar / STEP))
    if c:
        for x, y, _ in ref:
            for t in (int(round(x / STEP)), int(round(y / STEP))):
                scored[max(0, t - c) : t + c] = False
    R, H = R[:, scored], H[:, scored]
    # Overlap of every reference and hypothesis speaker, then the best one to one map.
    co = R.astype(np.int64) @ H.T.astype(np.int64) if len(hs) else np.zeros((len(rs), 0))
    rows, cols = linear_sum_assignment(-co) if co.size else ([], [])
    correct = np.zeros(R.shape[1], dtype=np.int64)
    for i, j in zip(rows, cols):
        correct += R[i] & H[j]
    nr, nh = R.sum(0), H.sum(0)
    total = nr.sum()
    miss = np.maximum(nr - nh, 0).sum()
    fa = np.maximum(nh - nr, 0).sum()
    conf = (np.minimum(nr, nh) - correct).sum()
    der = (miss + fa + conf) / total
    print(
        f"DER {der * 100:5.1f}%  miss {miss / total * 100:4.1f}  fa {fa / total * 100:4.1f}  "
        f"conf {conf / total * 100:4.1f}  speakers ref {len(rs)} hyp {len(hs)}"
    )


if __name__ == "__main__":
    main()
