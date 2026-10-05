// One production (Nitro node-server) process per core (what express/fastify do in TFB), sharing the port.
import cluster from 'node:cluster';
import os from 'node:os';
if (cluster.isPrimary) {
  for (let i = 0; i < os.availableParallelism(); i++) cluster.fork();
} else {
  await import('./.output/server/index.mjs');
}
