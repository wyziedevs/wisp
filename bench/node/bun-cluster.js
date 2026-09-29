// Runs a Bun server once per core. Bun has no cluster round-robin: each
// process binds the port with reusePort (SO_REUSEPORT, Linux) and the
// kernel spreads connections, as Bun's docs do it.
//
//   bun bun-cluster.js <server.js>     WORKERS sets the process count
const workers = Number(process.env.WORKERS) || navigator.hardwareConcurrency;
const procs = [];
for (let i = 0; i < workers; i++) {
    procs.push(Bun.spawn([process.execPath, process.argv[2]], { stdio: ['inherit', 'inherit', 'inherit'] }));
}
const code = await Promise.race(procs.map((p) => p.exited));
console.error(`a worker exited with ${code}`);
for (const p of procs) p.kill();
process.exit(1);
