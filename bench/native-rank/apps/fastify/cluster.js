// One worker per core (as Fastify's docs and TechEmpower's entry do), sharing the port.
const cluster = require('node:cluster');
const os = require('node:os');
if (cluster.isPrimary) {
  for (let i = 0; i < os.availableParallelism(); i++) cluster.fork();
} else {
  require('./app.js');
}
