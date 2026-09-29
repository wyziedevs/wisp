// Express with its defaults (ETag and X-Powered-By on), as apps run it;
// the page from a template literal (fortunes.mjs).
import express from 'express';
import { render } from '../fortunes.mjs';

const app = express();
app.get('/plaintext', (req, res) => res.type('text/plain').send('Hello, World!'));
app.get('/fortunes', (req, res) => res.type('text/html').send(render()));
app.get('/json', (req, res) => res.json({ message: 'Hello, World!' }));
app.listen(Number(process.env.PORT), '127.0.0.1');
