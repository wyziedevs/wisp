// One production (standalone) server per core sharing the port.
const cluster = require('node:cluster');
const os = require('node:os');
if (cluster.isPrimary) {
  for (let i = 0; i < os.availableParallelism(); i++) cluster.fork();
} else {
  process.chdir(__dirname + '/.next/standalone');
  require(__dirname + '/.next/standalone/server.js');
}
