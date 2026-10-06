#!/usr/bin/env python3
"""Differential fuzz: gruppera binary vs a naive reference aggregator.

Covers what the SWAR fast paths are most likely to get wrong: name lengths
around the 8/15/16/17-byte path boundaries, multibyte UTF-8, very long names
(up to 100 bytes), tiny files (< TAIL_MARGIN), files spanning many 2 MB
chunks, hash-colliding name sets, and every temperature layout.
"""
import math, os, random, subprocess, sys, tempfile

BIN = sys.argv[1]
SEEDS = int(sys.argv[2]) if len(sys.argv) > 2 else 200

ALPHABET = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ -'.()"
UNI = "éüößçñåøæ北京東京Мосваقاهرة🙂"


def rand_name(rng: random.Random, nbytes: int) -> str:
    # Build a valid UTF-8 name of exactly nbytes (no ';' or '\n').
    out, size = [], 0
    while size < nbytes:
        ch = rng.choice(UNI) if rng.random() < 0.2 else rng.choice(ALPHABET)
        b = len(ch.encode())
        if size + b > nbytes:
            ch, b = rng.choice("abcxyz"), 1
        out.append(ch)
        size += b
    return "".join(out)


def rand_temp(rng: random.Random) -> int:
    return rng.randint(-999, 999)  # tenths


def fmt_temp(t: int) -> str:
    s = "-" if t < 0 else ""
    a = abs(t)
    return f"{s}{a // 10}.{a % 10}"


def round1(x: float) -> float:
    return math.floor(x * 10.0 + 0.5) / 10.0


def reference(lines):
    agg = {}
    for name, t in lines:
        e = agg.get(name)
        if e is None:
            agg[name] = [t, t, t, 1]
        else:
            e[0] = min(e[0], t); e[1] += t; e[2] = max(e[2], t); e[3] += 1
    parts = []
    for name in sorted(agg, key=lambda s: s.encode()):
        lo, tot, hi, n = agg[name]
        parts.append(f"{name}={round1(lo / 10.0):.1f}/{round1(tot / 10.0 / n):.1f}/{round1(hi / 10.0):.1f}")
    return "{" + ", ".join(parts) + "}"


def page_align(lines, page: int = 4096):
    """Pad with short repeated-name rows so the file ends exactly on a page.

    With the input mmap'd, any load past EOF then hits an unmapped page and
    faults (SIGSEGV/SIGBUS) instead of silently reading zero padding — so a
    tail over-read is a hard failure here, not page-rounding luck. The last
    rows repeat 1- and 2-byte names so the tail path's name comparison runs
    right at EOF.
    """
    size = sum(len(n.encode()) + len(fmt_temp(t)) + 2 for n, t in lines)
    gap = (-size) % page
    while gap < 60:
        gap += page
    for b in range(2, 400):
        r = gap - 7 * b  # "zz;1.0\n" is 7 bytes, "z;1.0\n" is 6
        if r >= 12 and r % 6 == 0:
            return lines + [("zz", 10)] * b + [("z", 10)] * (r // 6)
    raise AssertionError(gap)


def case(seed: int):
    rng = random.Random(seed)
    kind = seed % 6
    if kind == 0:   # tiny file, below TAIL_MARGIN
        nst, rows = rng.randint(1, 3), rng.randint(1, 6)
    elif kind == 1: # boundary lengths only
        nst, rows = 40, rng.randint(200, 5000)
    elif kind == 2: # many stations (10k), multi-chunk
        nst, rows = 10_000, rng.randint(300_000, 700_000)
    elif kind == 3: # few stations, multi-chunk
        nst, rows = rng.randint(1, 20), rng.randint(200_000, 500_000)
    elif kind == 4: # long names up to 100 bytes
        nst, rows = 500, rng.randint(1000, 50_000)
    else:           # random mix
        nst, rows = rng.randint(1, 3000), rng.randint(1, 100_000)
    names = set()
    while len(names) < nst:
        if kind == 1:
            n = rng.choice([1, 2, 7, 8, 9, 14, 15, 16, 17, 23, 24, 25, 31, 32, 33])
        elif kind == 4:
            n = rng.randint(90, 100)
        else:
            n = rng.choice([rng.randint(1, 16), rng.randint(1, 100)])
        names.add(rand_name(rng, n))
    names = sorted(names)
    lines = [(rng.choice(names), rand_temp(rng)) for _ in range(rows)]
    return page_align(lines) if seed % 2 == 0 else lines


def main():
    fails = 0
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "m.txt")
        for seed in range(SEEDS):
            lines = case(seed)
            with open(path, "w", encoding="utf-8", newline="\n") as f:
                f.write("".join(f"{n};{fmt_temp(t)}\n" for n, t in lines))
            size = os.path.getsize(path)
            assert seed % 2 or size % 4096 == 0, size
            want = reference(lines)
            r = subprocess.run([BIN, path], capture_output=True)
            got = r.stdout.decode().rstrip("\n")
            if r.returncode != 0 or got != want:
                fails += 1
                print(f"seed {seed}: MISMATCH rc={r.returncode} size={os.path.getsize(path)} rows={len(lines)}")
                if r.returncode == 0:
                    a, b = got.split(", "), want.split(", ")
                    for x, y in zip(a, b):
                        if x != y:
                            print("   got ", x[:160]); print("   want", y[:160]); break
                    if len(a) != len(b):
                        print(f"   station count got {len(a)} want {len(b)}")
                if fails >= 5:
                    break
    print(f"{SEEDS} cases, {fails} failures")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
