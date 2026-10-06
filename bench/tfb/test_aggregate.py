"""Fixture tests of aggregate.parse: python3 bench/tfb/test_aggregate.py"""
import tempfile, unittest
from pathlib import Path
import aggregate

def run(body):
    p = Path(tempfile.mkdtemp()) / "x.txt"
    p.write_text("Running 15s test @ http://h/\n" + body)
    return aggregate.parse(p)

OK = """    Latency     1.50ms    0.20ms  10.00ms   90%
     99%    2.00ms
  1000 requests in 15.01s, 1.00MB read
Requests/sec:     66.62
"""

class Parse(unittest.TestCase):
    def test_ok(self):
        r = run(OK)
        self.assertAlmostEqual(r["rps"], 66.62); self.assertEqual(r["lat_avg_ms"], 1.5)
        self.assertFalse(r["stalled"])
    def test_units(self):
        self.assertEqual(aggregate.ms("250.00us"), 0.25); self.assertEqual(aggregate.ms("1.20s"), 1200)
        self.assertIsNone(aggregate.ms("0.00us"))  # pipelined p99
    def test_non2xx_scaled(self):
        r = run(OK + "  Non-2xx or 3xx responses: 250\n")
        self.assertAlmostEqual(r["rps"], 66.62 * 0.75); self.assertEqual(r["non2xx"], 250)
    def test_socket(self):
        r = run(OK + "  Socket errors: connect 1, read 2, write 3, timeout 4\n")
        self.assertEqual(r["socket_errors"], 10)
    def test_stall_zero(self):
        r = run(OK.replace("1000 requests in 15.01s", "0 requests in 22.40s").replace("66.62", "0.00"))
        self.assertTrue(r["stalled"])
    def test_stall_wall(self):
        self.assertTrue(run(OK.replace("15.01s", "0.91m"))["stalled"])
    def test_dead_after(self):
        self.assertTrue(run(OK + "# alive-after: no\n")["stalled"])
        self.assertFalse(run(OK + "# alive-after: yes\n")["stalled"])
    def test_no_hand_written_failures(self):
        # A failed cell comes from the data ("no request completed"), never a per-contender note.
        self.assertFalse(hasattr(aggregate, "FAILED"))
    def test_disturbed_on_every_retry(self):
        self.assertIsNone(run(OK + "# NOTE: foreign CPU or steal stayed high on every attempt\n"))
    def test_no_latency(self):
        self.assertIsNone(run("Requests/sec: 5\n"))

class Limited(unittest.TestCase):
    def row(self, **kw):
        return {"failed": None, "rps_max": 100.0, "requests": 1000, "non2xx": 0, **kw}
    def test_only_the_top_level(self):
        self.assertFalse(aggregate.limited(4096, self.row(failed="no request completed", rps_max=0)))
        self.assertTrue(aggregate.limited(16384, self.row(failed="no request completed", rps_max=0)))
    def test_non2xx_dominated(self):
        self.assertTrue(aggregate.limited(16384, self.row(non2xx=600)))
        self.assertFalse(aggregate.limited(16384, self.row(non2xx=100)))
    def test_clean_cell(self):
        self.assertFalse(aggregate.limited(16384, self.row()))

def cell(rps, steal=0.5, drained=True):
    return {"rps": rps, "lat_avg_ms": 1.0, "lat_p99_ms": 2.0, "non2xx": 0, "socket_errors": 0, "requests": 1000,
            "steal_pct": steal, "drained": drained, "stalled": False}

def table(**rows):
    return aggregate.summarize({c: {"json": {64: runs}} for c, runs in rows.items()})["json"][64]

class Fairness(unittest.TestCase):
    def test_drain_marker_parsed(self):
        self.assertTrue(run("# drain-before: 2s\n" + OK)["drained"])
        self.assertFalse(run(OK)["drained"])
    def test_a_run_over_the_steal_bar_is_not_ranked_or_tied(self):
        rows = table(a=[cell(100), cell(101), cell(102)], b=[cell(100), cell(101, steal=17.6), cell(102)], c=[cell(100), cell(101), cell(102)])
        self.assertTrue(rows["b"]["disturbed"]); self.assertFalse(aggregate.rankable(rows["b"]))
        self.assertEqual(rows["a"]["tied_with"], ["c"])   # b takes no part
    def test_steal_is_per_run_not_the_mean(self):
        rows = table(a=[cell(100, 0.0), cell(100, 0.0), cell(100, 10.5)])
        self.assertTrue(rows["a"]["disturbed"])
    def test_steal_exactly_at_the_bar_is_valid(self):
        self.assertFalse(table(a=[cell(1, 10.0)] * 3)["a"]["disturbed"])
    def test_three_valid_runs_to_rank(self):
        rows = table(a=[cell(100), cell(101)], b=[cell(100), cell(101), cell(102)])
        self.assertFalse(aggregate.rankable(rows["a"])); self.assertTrue(aggregate.rankable(rows["b"]))
        self.assertEqual(rows["b"]["tied_with"], [])
    def test_stalled_runs_do_not_count_as_valid(self):
        bad = dict(cell(5), stalled=True)
        rows = table(a=[cell(100), cell(101), bad])
        self.assertEqual(rows["a"]["runs"], 2); self.assertFalse(aggregate.rankable(rows["a"]))
    def test_stale_harness_from_data(self):
        rows = table(a=[cell(1, drained=False)] * 3, b=[cell(1)] * 3)
        self.assertTrue(rows["a"]["pre_drain"]); self.assertFalse(rows["b"]["pre_drain"])

class Lead(unittest.TestCase):
    def test_unpublished_rival_is_not_a_win(self):
        wisp = [cell(500), cell(501), cell(502)]
        full = aggregate.summarize({"wisp": {"json": {64: wisp}}, "axum": {"json": {64: [cell(100)] * 3}}})
        self.assertEqual(aggregate.lead_counts(full), (1, 1, 0, 0, 0))
        hid = aggregate.summarize({"wisp": {"json": {64: wisp}}, "axum": {"json": {64: [cell(100), cell(100, steal=30), cell(100)]}}})
        self.assertEqual(aggregate.lead_counts(hid), (0, 1, 0, 0, 1))

if __name__ == "__main__":
    unittest.main()
