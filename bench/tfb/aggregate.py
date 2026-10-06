#!/usr/bin/env python3
"""Reads raw/<contender>/<workload>-c<level>-run<N>.txt (wrk output) and writes results.json
and the tables of RESULTS.md (stdout). Median/min/max of requests/sec; latency avg and p99 are
the medians of the runs. req/s counts only 2xx/3xx responses; runs with non-2xx responses or
socket errors are flagged, not dropped. A stalled run (no request completed, or wall time over
1.5x the test duration) is left out of the median/min/max and counted in the errors column."""
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
HIGH_LEVEL = 16384
STEAL_MAX = 10.0   # percent of all CPU ticks; the same bar run.sh retries on
MIN_RUNS = 3       # valid runs a cell needs before it is ranked or tied
LIMITED = "load-generator limited (local port range)"
UNIT = {"us": 1e-3, "ms": 1.0, "s": 1000.0, "m": 60000.0}


def ms(text):
    m = re.match(r"([\d.]+)(us|ms|s|m)$", text)
    v = float(m.group(1)) * UNIT[m.group(2)] if m else None
    return v or None   # wrk prints 0.00us for percentiles it could not compute (pipelining)


def limited(lvl, row):
    """At the top level the client, not the server, is the limit: wrk's connect() slows on the VM's
    ~28k local ports, so a cell with no completed request, or mostly non-2xx replies, says nothing
    about the server. Derived from the data only."""
    if lvl < HIGH_LEVEL:
        return False
    bad = row["failed"] or row["rps_max"] == 0
    return bool(bad or (row["requests"] and 2 * row["non2xx"] > row["requests"]))


def parse(path):
    t = path.read_text(errors="replace")
    rps = re.search(r"Requests/sec:\s+([\d.]+)", t)
    avg = re.search(r"Latency\s+([\d.]+(?:us|ms|s|m))", t)
    p99 = re.search(r"99%\s+([\d.]+(?:us|ms|s|m))", t)
    non2 = re.search(r"Non-2xx or 3xx responses: (\d+)", t)
    sock = re.search(r"Socket errors: connect (\d+), read (\d+), write (\d+), timeout (\d+)", t)
    total = re.search(r"(\d+) requests in", t)
    if not (rps and avg and p99) or "# NOTE: foreign CPU or steal stayed high" in t:  # unusable, or disturbed on every retry
        return None
    dur = re.search(r"Running (\d+)s test", t)
    wall = re.search(r"requests in ([\d.]+)(us|ms|s|m),", t)
    wall_s = float(wall.group(1)) * UNIT[wall.group(2)] / 1000 if wall else None
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
        "drained": "# drain-before" in t,   # run by a run.sh that waits for the server to go idle first
        "stalled": n_total == 0 or "# alive-after: no" in t or bool(dur and wall_s and wall_s > 1.5 * int(dur.group(1))),
    }


def rankable(r):
    """A cell is published and ranked only from MIN_RUNS valid runs, none disturbed by hypervisor steal."""
    return not r["failed"] and not r["disturbed"] and r["runs"] >= MIN_RUNS


def summarize(contenders):
    summary = {}
    for c, ws in contenders.items():
        for w, lv in ws.items():
            for lvl, every in lv.items():
                runs = [r for r in every if not r["stalled"]] or every
                rps = [r["rps"] for r in runs]
                summary.setdefault(w, {}).setdefault(lvl, {})[c] = {
                    "runs": len(runs), "runs_total": len(every), "stalled": sum(r["stalled"] for r in every), "rps_median": statistics.median(rps), "rps_min": min(rps),
                    "rps_max": max(rps),
                    "lat_avg_ms_median": statistics.median([r["lat_avg_ms"] for r in runs if r["lat_avg_ms"]] or [0]),
                    "lat_p99_ms_median": (statistics.median([r["lat_p99_ms"] for r in runs if r["lat_p99_ms"]]) if any(r["lat_p99_ms"] for r in runs) else None),
                    "non2xx": sum(r["non2xx"] for r in runs),
                    "requests": sum(r["requests"] or 0 for r in runs),
                    "socket_errors": sum(r["socket_errors"] for r in runs),
                    "steal_pct_max": max((r["steal_pct"] or 0) for r in runs),
                    "disturbed": any((r["steal_pct"] or 0) > STEAL_MAX for r in runs),
                    "pre_drain": any(not r["drained"] for r in runs),
                }
    # A level where no run completed a request is a failed run, not a 0. Ranges that overlap are
    # ties: a row gets a rank only from MIN_RUNS valid runs, each with steal <= STEAL_MAX, and only when its
    # min-max range overlaps no other row's (supplementary, failed and disturbed rows take no part).
    for w, lv in summary.items():
        for lvl, rows in lv.items():
            for c, r in rows.items():
                r["failed"] = ("no request completed" if r["rps_max"] == 0
                               else "every run stalled" if r["stalled"] == r["runs_total"] else None)
            for r in rows.values():
                r["limited"] = limited(lvl, r)
            ranked = [c for c in rows if c != "wisp-uncapped" and rankable(rows[c])]
            for c, r in rows.items():
                r["tied_with"] = sorted(o for o in ranked if c in ranked and o != c
                                        and r["rps_min"] <= rows[o]["rps_max"] and rows[o]["rps_min"] <= r["rps_max"])
    return summary


def lead_counts(summary):
    """Wisp lead counts per level: (wins, levels, ties, leads with errors, leads with an unpublished rival)."""
    firsts, tot, tied, flagged, partial = 0, 0, 0, 0, 0
    for w in ("plaintext", "json"):
        for lvl in sorted(summary.get(w, {})):
            rows = {c: r for c, r in summary[w][lvl].items() if c != "wisp-uncapped" and rankable(r)}
            if "wisp" not in rows:
                continue
            lead = max(rows, key=lambda c: rows[c]["rps_median"])
            top = {lead} | set(rows[lead]["tied_with"])
            tot += 1
            w0 = rows["wisp"]
            clean = not (w0["stalled"] or w0["non2xx"] or w0["socket_errors"])
            # A rival whose cell is unpublished (steal, too few runs) cannot be beaten on this data: not a win.
            hidden = any(c not in ("wisp", "wisp-uncapped") and not r["failed"] and not rankable(r) for c, r in summary[w][lvl].items())
            partial += top == {"wisp"} and clean and hidden
            firsts += top == {"wisp"} and clean and not hidden
            flagged += top == {"wisp"} and not clean
            tied += len(top) > 1 and "wisp" in top
    return firsts, tot, tied, flagged, partial


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
    summary = summarize(out["contenders"])
    out["summary"] = summary
    (ROOT / "results.json").write_bytes(json.dumps(out, indent=1, sort_keys=True).encode())

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

    # Every contender, every workload, every level, every metric. Sort key: median req/s, descending,
    # failed rows last. Tied rows show "tie" and whom with, never a rank.
    lines = []
    for w, title in (("plaintext", "Plaintext (pipeline depth 16)"), ("json", "JSON serialization")):
        for lvl in sorted(summary.get(w, {})):
            rows = summary[w][lvl]
            order = sorted(rows, key=lambda c: (bool(rows[c]["failed"]), bool(rows[c]["disturbed"]), -rows[c]["rps_median"]))
            ranked = [c for c in order if c != "wisp-uncapped" and rankable(rows[c])]
            lines.append(f"\n#### {title}, {lvl} connections\n")
            lines.append("| # | Contender | req/s median | min | max | latency avg (ms) | latency p99 (ms) | errors | steal % max | tied with |")
            lines.append("|---:|---|---:|---:|---:|---:|---:|---|---:|---|")
            for c in order:
                r = rows[c]
                tied = ", ".join(LABEL[o].split(" (")[0] for o in r["tied_with"]) or "-"
                rank = "tie" if r["tied_with"] else str(ranked.index(c) + 1) if c in ranked else "-"
                err = []
                if r["non2xx"]:
                    err.append(f"{r['non2xx']} non-2xx")
                if r["runs"] < MIN_RUNS and not r["failed"]:
                    err.append(f"{r['runs']} valid run(s), {MIN_RUNS} needed, not ranked")
                if r["pre_drain"]:
                    err.append("pre-drain harness (no drain-before in raw), steal not controlled")
                if r["stalled"]:
                    err.append(f"{r['stalled']} stalled run(s) left out")
                if r["socket_errors"]:
                    err.append(f"{r['socket_errors']} socket")
                if r["limited"]:
                    err.append(LIMITED + ", a measurement caveat, not a server failure")
                if r["disturbed"]:
                    cells = [rank, LABEL[c], f"Not published (a run had steal over {STEAL_MAX:g}%)", "-", "-", "-", "-",
                             ', '.join(err) or '-', f"{r['steal_pct_max']:.1f}", "-"]
                elif r["failed"]:
                    cells = [rank, LABEL[c], f"Failed ({r['failed']})", "-", "-", "-", "-",
                             ', '.join(err) or '-', f"{r['steal_pct_max']:.1f}", "-"]
                else:
                    cells = [rank, LABEL[c], f(r['rps_median']), f(r['rps_min']), f(r['rps_max']),
                             l(r['lat_avg_ms_median']), l(r['lat_p99_ms_median']), ', '.join(err) or '-',
                             f"{r['steal_pct_max']:.1f}", tied]
                lines.append("| " + " | ".join(bold(c, x) for x in cells) + " |")
            lines.append(f"\nRows whose min-max ranges overlap are ties and get no rank. A cell is ranked only from {MIN_RUNS} valid runs, "
                         f"each with steal at most {STEAL_MAX:g}% of CPU; a cell with a run above that is not published.")
    tables = "\n".join(lines)

    # Derived summary: counts only, from the data above (supplementary rows excluded).
    firsts, tot, tied, flagged, partial = lead_counts(summary)
    cells_all = [r for w in summary.values() for lv in w.values() for r in lv.values()]
    stale = sum(r["pre_drain"] for r in cells_all)
    dist = sum(r["disturbed"] for r in cells_all)
    heads = (f"Wisp (defaults) has a min-max range above every other contender's at {firsts} of {tot} "
             f"workload and connection levels; at {tied} more its range overlaps that of the highest "
             f"median (a tie within noise)"
             + (f"; at {partial} more it leads every published contender but at least one rival's cell is unpublished, so that is not counted as a win" if partial else "")
             + (f"; at {flagged} more it leads but its row has stalled runs or errors, so that is not counted as a win" if flagged else "")
             + ". Counted from the tables below, supplementary row excluded."
             + (f" {dist} of {len(cells_all)} cells have a run with steal over {STEAL_MAX:g}% and are not published or ranked." if dist else "")
             + (f" {stale} of {len(cells_all)} cells come from the pre-drain harness (no drain-before in their raw files): steal was not controlled by waiting for the server to go idle, so rank comparisons with drained rows are not like for like until a full re-run." if stale else ""))

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
        (ROOT / "RESULTS.md").write_bytes(md.encode())
    print(heads)
    print(tables)


if __name__ == "__main__":
    main()
