#!/usr/bin/env python3
import subprocess, sys, time
b = sys.argv[1] if len(sys.argv) > 1 else "./target/release/gruppera"
f = sys.argv[2] if len(sys.argv) > 2 else "data/measurements.txt"
n = int(sys.argv[3]) if len(sys.argv) > 3 else 5
best = float('inf')
for i in range(n):
    t0 = time.perf_counter()
    subprocess.run([b, f], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
    dt = time.perf_counter() - t0
    print(f"run {i+1}: {dt:.3f}s", flush=True)
    best = min(best, dt)
print(f"BEST: {best:.3f}s")
