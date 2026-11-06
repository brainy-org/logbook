# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""The reference of a recording, who spoke when, made by hand in Audacity.

uv run scripts/reference.py draft HYP.rttm > labels.txt
uv run scripts/reference.py rttm labels.txt [--file NAME] > ref.rttm
uv run scripts/reference.py check labels.txt
uv run scripts/reference.py offset LABELLED.wav OTHER.wav

`draft` turns a diarization (an RTTM of `diarize`, e.g. the kept strategy)
into an Audacity label track: a label per turn, `spk0`, `spk1`... Import it
over the recording (File > Import > Labels), listen through, and fix it:
rename each label to the person (any name, the same spelling every time),
move its edges to where the voice starts and ends, split, merge, add the
turns it missed and delete what is not speech. Two people at once: one label
`Anna+Marco`, or two labels that overlap. A label `?` (speech whose speaker is
not known) is left out of the reference and counted. Then File > Export >
Labels.

`rttm` turns that track into the reference RTTM (`scripts/der.py`); with
`--offset S` its times are moved by S seconds, for another recording of the
same meeting that started earlier or later. `check`
prints the speakers and their seconds, and what is suspicious: a label still
`spkN`, a label of under 0.2 s, a gap of over 30 s.

`offset` says how many seconds to add to LABELLED.wav's times to get
OTHER.wav's (two devices recording the same meeting, started apart: a clap at
the start helps), from the cross-correlation of their loudness, 10 ms steps.

The draft comes from one diarizer: corrected by ear it is a fair reference
for who spoke (confusion), but its edges are the diarizer's wherever they were
not moved, so missed speech and false alarm are a little in that diarizer's
favour. der.py's 0.25 s collar absorbs most of it; score confusion first.
"""

import argparse
import re
import subprocess
import sys

STRAY = re.compile(r"^spk\d+$")


def read_rttm(path):
    turns = []
    for line in open(path):
        f = line.split()
        if len(f) >= 8 and f[0] == "SPEAKER":
            start, dur = float(f[3]), float(f[4])
            turns.append((start, start + dur, f[7]))
    return sorted(turns)


def read_labels(path):
    """Audacity's label track: `start<TAB>end<TAB>label`; its spectral lines (`\\`) skipped."""
    labels = []
    for n, line in enumerate(open(path, encoding="utf-8"), 1):
        line = line.rstrip("\n")
        if not line.strip() or line.startswith("\\"):
            continue
        f = line.split("\t")
        if len(f) < 2:
            sys.exit(f"{path}:{n}: not start<TAB>end<TAB>label: {line!r}")
        start, end = float(f[0].replace(",", ".")), float(f[1].replace(",", "."))
        label = f[2].strip() if len(f) > 2 else ""
        if end < start:
            sys.exit(f"{path}:{n}: ends before it starts")
        labels.append((start, end, label))
    return sorted(labels)


def speakers_of(label):
    """The speakers of a label: none for `?` or an empty one."""
    if label in ("", "?"):
        return []
    return [s.strip().lower().replace(" ", "_") for s in label.split("+") if s.strip()]


def draft(a):
    for start, end, spk in read_rttm(a.hyp):
        print(f"{start:.3f}\t{end:.3f}\t{spk}")


def rttm(a):
    labels = [(s + a.offset, e + a.offset, l) for s, e, l in read_labels(a.labels)]
    if labels and labels[0][0] < 0:
        print("labels before the recording's start dropped", file=sys.stderr)
        labels = [(max(s, 0.0), e, l) for s, e, l in labels if e > 0]
    name = a.file or re.sub(r"\.[^.]*$", "", a.labels.split("/")[-1])
    unknown = 0.0
    for start, end, label in labels:
        spks = speakers_of(label)
        if not spks:
            unknown += end - start
        for s in spks:
            print(f"SPEAKER {name} 1 {start:.3f} {end - start:.3f} <NA> <NA> {s} <NA> <NA>")
    if unknown:
        print(f"left out: {unknown:.1f} s of speech labelled '?' or unlabelled", file=sys.stderr)


def check(a):
    labels = read_labels(a.labels)
    secs = {}
    problems = []
    last = 0.0
    for start, end, label in labels:
        if STRAY.match(label):
            problems.append(f"{start:8.2f}  still a draft label: {label}")
        if end - start < 0.2:
            problems.append(f"{start:8.2f}  {end - start:.2f} s: too short to be a turn?")
        if start - last > 30:
            problems.append(f"{last:8.2f}  {start - last:.0f} s without a label")
        last = max(last, end)
        for s in speakers_of(label) or ["?"]:
            secs[s] = secs.get(s, 0.0) + end - start
    for s, t in sorted(secs.items(), key=lambda x: -x[1]):
        print(f"{s:20} {t:7.1f} s")
    print(f"{len(labels)} labels, to {last:.1f} s")
    for p in problems:
        print(p)


def loudness(path, secs):
    """Log energy of 10 ms frames of the first `secs` seconds, mean removed."""
    import numpy as np

    raw = subprocess.run(
        ["ffmpeg", "-loglevel", "error", "-t", str(secs), "-i", path, "-ar", "16000", "-ac", "1", "-f", "s16le", "-"],
        capture_output=True,
        check=True,
    ).stdout
    x = np.frombuffer(raw, dtype=np.int16).astype(np.float64) / 32768
    n = len(x) // 160
    e = np.log((x[: n * 160].reshape(n, 160) ** 2).mean(1) + 1e-8)
    return e - e.mean()


def offset(a):
    import numpy as np

    x, y = loudness(a.labelled, a.secs), loudness(a.other, a.secs)
    n = 1 << int(np.ceil(np.log2(len(x) + len(y))))
    c = np.fft.irfft(np.fft.rfft(y, n) * np.conj(np.fft.rfft(x, n)), n)
    lags = np.concatenate([np.arange(0, len(y)), np.arange(-len(x) + 1, 0)])
    c = np.concatenate([c[: len(y)], c[n - len(x) + 1 :]])
    best = int(np.argmax(c))
    peak = c[best] / (np.linalg.norm(x) * np.linalg.norm(y) + 1e-12)
    print(f"{lags[best] * 0.01:.2f}")
    print(f"correlation {peak:.2f} (under 0.3: not the same meeting, or too far apart)", file=sys.stderr)


def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    d = sub.add_parser("draft", help="an RTTM as an Audacity label track")
    d.add_argument("hyp")
    d.set_defaults(run=draft)
    r = sub.add_parser("rttm", help="an Audacity label track as the reference RTTM")
    r.add_argument("labels")
    r.add_argument("--file", help="the recording's name in the RTTM (default: the labels' file name)")
    r.add_argument("--offset", type=float, default=0.0, help="seconds added to every time")
    r.set_defaults(run=rttm)
    c = sub.add_parser("check", help="speakers, seconds and suspicious labels")
    c.add_argument("labels")
    c.set_defaults(run=check)
    o = sub.add_parser("offset", help="seconds from one recording of a meeting to another")
    o.add_argument("labelled")
    o.add_argument("other")
    o.add_argument("--secs", type=float, default=300, help="how much of each to compare")
    o.set_defaults(run=offset)
    a = p.parse_args()
    a.run(a)


if __name__ == "__main__":
    main()
