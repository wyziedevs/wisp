// Runs a Node server once per core, as pm2 -i and TechEmpower's Node
// entries do: Node runs JavaScript on one thread, so a single process would
// use one of the cores the other servers get.
//
//   node cluster.mjs <server.js>     WORKERS sets the process count
import cluster from 'node:cluster';
import { availableParallelism } from 'node:os';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

if (cluster.isPrimary) {
    // Round-robin, the default everywhere but Windows, spreads the load
    // generator's connections evenly instead of leaving it to the OS.
    cluster.schedulingPolicy = cluster.SCHED_RR;
    const workers = Number(process.env.WORKERS) || availableParallelism();
    for (let i = 0; i < workers; i++) cluster.fork();
    cluster.on('exit', (w, code) => { console.error(`worker ${w.process.pid} exited with ${code}`); process.exit(1); });
} else {
    await import(pathToFileURL(resolve(process.argv[2])).href);
}
