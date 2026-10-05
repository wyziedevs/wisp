#!/usr/bin/env python3
"""Reads raw/<contender>/<workload>-c<level>-run<N>.txt (wrk output) and writes results.json
and the tables of RESULTS.md (stdout). Median/min/max of requests/sec; latency avg and p99 are
the medians of the runs. req/s counts only 2xx/3xx responses; runs with non-2xx responses or
socket errors are flagged, not dropped."""
import json, re, statistics, sys
from pathlib import Path

ROOT = Path(__file__).parent
RAW = ROOT / "raw"
ORDER = ["wisp", "wisp-uncapped", "axum", "actix", "express", "fastify", "hono-node", "hono-bun", "sveltekit", "next", "nuxt"]
LABEL = {
    "wisp": "Wisp (defaults)", "wisp-uncapped": "Wisp, WISP_MAX_CONNS=0 (supplementary)", "axum": "Axum (TFB source)", "actix": "Actix Web (TFB source)",
    "express": "Express (TFB source)", "fastify": "Fastify (TFB source)",
    "hono-node": "Hono on Node (TFB source)", "hono-bun": "Hono on Bun (not TFB source)",
    "sveltekit": "SvelteKit (not TFB)", "next": "Next.js (not TFB)", "nuxt": "Nuxt (not TFB)",
}
UNIT = {"us": 1e-3, "ms": 1.0, "s": 1000.0, "m": 60000.0}


def ms(text):
    m = re.match(r"([\d.]+)(us|ms|s|m)$", text)
    v = float(m.group(1)) * UNIT[m.group(2)] if m else None
    return v or None   # wrk prints 0.00us for percentiles it could not compute (pipelining)


def parse(path):
    t = path.read_text(errors="replace")
    rps = re.search(r"Requests/sec:\s+([\d.]+)", t)
    avg = re.search(r"Latency\s+([\d.]+(?:us|ms|s|m))", t)
    p99 = re.search(r"99%\s+([\d.]+(?:us|ms|s|m))", t)
    non2 = re.search(r"Non-2xx or 3xx responses: (\d+)", t)
    sock = re.search(r"Socket errors: connect (\d+), read (\d+), write (\d+), timeout (\d+)", t)
    total = re.search(r"(\d+) requests in", t)
    if not (rps and avg and p99):  # a run without latency lines is unusable
        return None
    cpu = re.search(r"# cpu-delta[^:]*: ([\d ]+)", t)
    steal = None
    if cpu:
        v = [int(x) for x in cpu.group(1).split()]
        steal = round(100 * v[7] / max(1, sum(v)), 2)
    n_total = int(total.group(1)) if total else None
    n_bad = int(non2.group(1)) if non2 else 0
    # Only successful responses count: wrk's Requests/sec includes non-2xx replies.
    ok_rps = float(rps.group(1)) * (n_total - n_bad) / n_total if n_total else float(rps.group(1))
    return {
        "rps": ok_rps, "lat_avg_ms": ms(avg.group(1)), "lat_p99_ms": ms(p99.group(1)),
        "non2xx": int(non2.group(1)) if non2 else 0,
        "socket_errors": sum(int(x) for x in sock.groups()) if sock else 0,
        "requests": int(total.group(1)) if total else None, "steal_pct": steal,
    }


def main():
    out = {"contenders": {}, "omitted": {}}
    for c in ORDER:
        d = RAW / c
        if (d / "OMITTED").exists():
            out["omitted"][c] = (d / "OMITTED").read_text().strip()
            continue
        for f in sorted(d.glob("*-c*-run*.txt")):
            m = re.match(r"(plaintext|json)-c(\d+)-run(\d+)\.txt", f.name)
            r = parse(f)
            if not m or r is None:
                continue
            w, lvl, run = m.group(1), int(m.group(2)), int(m.group(3))
            out["contenders"].setdefault(c, {}).setdefault(w, {}).setdefault(lvl, []).append(r)
    summary = {}
    for c, ws in out["contenders"].items():
        for w, lv in ws.items():
            for lvl, runs in lv.items():
                rps = [r["rps"] for r in runs]
                summary.setdefault(w, {}).setdefault(lvl, {})[c] = {
                    "runs": len(runs), "rps_median": statistics.median(rps), "rps_min": min(rps),
                    "rps_max": max(rps),
                    "lat_avg_ms_median": statistics.median([r["lat_avg_ms"] for r in runs if r["lat_avg_ms"]] or [0]),
                    "lat_p99_ms_median": (statistics.median([r["lat_p99_ms"] for r in runs if r["lat_p99_ms"]]) if any(r["lat_p99_ms"] for r in runs) else None),
                    "non2xx": sum(r["non2xx"] for r in runs),
                    "socket_errors": sum(r["socket_errors"] for r in runs),
                    "steal_pct_max": max((r["steal_pct"] or 0) for r in runs),
                }
    out["summary"] = summary
    (ROOT / "results.json").write_text(json.dumps(out, indent=1, sort_keys=True))

    def f(n):
        return f"{n:,.0f}"

    def l(x):
        if x is None:
            return "n/a"
        return f"{x:.2f}" if x < 100 else f"{x:.0f}"

    def name(c):
        return f"**{LABEL[c]}**" if c.startswith("wisp") else LABEL[c]

    def bold(c, text):
        return f"**{text}**" if c.startswith("wisp") else text

    # Every contender, every workload, every level, every metric. Sort key: median req/s, descending.
    lines = []
    for w, title in (("plaintext", "Plaintext (pipeline depth 16)"), ("json", "JSON serialization")):
        for lvl in sorted(summary.get(w, {})):
            rows = summary[w][lvl]
            order = sorted(rows, key=lambda c: -rows[c]["rps_median"])
            lines.append(f"\n#### {title}, {lvl} connections\n")
            lines.append("| # | Contender | req/s median | min | max | latency avg (ms) | latency p99 (ms) | errors | steal % max |")
            lines.append("|---:|---|---:|---:|---:|---:|---:|---|---:|")
            for i, c in enumerate(order, 1):
                r = rows[c]
                err = []
                if r["non2xx"]:
                    err.append(f"{r['non2xx']} non-2xx")
                if r["socket_errors"]:
                    err.append(f"{r['socket_errors']} socket")
                cells = [str(i), LABEL[c], f(r['rps_median']), f(r['rps_min']), f(r['rps_max']),
                         l(r['lat_avg_ms_median']), l(r['lat_p99_ms_median']), ', '.join(err) or '-',
                         f"{r['steal_pct_max']:.1f}"]
                lines.append("| " + " | ".join(bold(c, x) for x in cells) + " |")
    tables = "\n".join(lines)

    # Derived summary: counts only, from the data above (supplementary rows excluded).
    firsts, tot, tied = 0, 0, 0
    for w in ("plaintext", "json"):
        for lvl in sorted(summary.get(w, {})):
            rows = {c: r for c, r in summary[w][lvl].items() if c != "wisp-uncapped"}
            if "wisp" not in rows:
                continue
            lead = max(rows, key=lambda c: rows[c]["rps_median"])
            tot += 1
            firsts += lead == "wisp"
            tied += lead != "wisp" and rows["wisp"]["rps_max"] >= rows[lead]["rps_min"]
    heads = (f"By median req/s Wisp (defaults) has the highest median at {firsts} of {tot} workload and "
             f"connection levels; at {tied} more its min-max range overlaps the leader's (a tie within "
             f"noise). Counted from the tables below, supplementary row excluded.")

    noise = ["| Binary | Level | round 1 | round 2 | round 3 | steal % of CPU (r1/r2/r3) |", "|---|---:|---:|---:|---:|---|"]
    for c in ("wisp", "wisp-uncapped"):
        for lvl in (16, 256):
            runs = [parse(RAW / "noise" / f"{c}-json-c{lvl}-r{r}.txt") for r in (1, 2, 3)]
            if all(runs):
                noise.append(f"| {LABEL[c]} | {lvl} | " + " | ".join(f(r["rps"]) for r in runs) + " | "
                             + " / ".join(str(r["steal_pct"]) for r in runs) + " |")
    noises = chr(10).join(noise) if len(noise) > 2 else "not run"

    tpl = ROOT / "RESULTS.template.md"
    if tpl.exists():
        env = (RAW / "env.txt").read_text() if (RAW / "env.txt").exists() else ""
        omitted = "\n".join(f"- {v}" for v in out["omitted"].values()) or "- none"
        md = (tpl.read_text().replace("{{HEADLINE}}", heads).replace("{{TABLES}}", tables)
              .replace("{{ENV}}", env).replace("{{NOISE}}", noises).replace("{{OMITTED}}", omitted))
        (ROOT / "RESULTS.md").write_text(md)
    print(heads)
    print(tables)


main()
