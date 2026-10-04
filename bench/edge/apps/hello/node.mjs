// The floor: node:http answering "hello" to everything, with nothing else in
// the way. No framework can be faster on Node's http module.
import { createServer } from 'node:http';

createServer((req, res) => res.end('hello')).listen(Number(process.env.PORT) || 4200);
