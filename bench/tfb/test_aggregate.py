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

if __name__ == "__main__":
    unittest.main()
