// @feature setup
import express from 'express';
import { items } from './db.js';

const app = express();
app.set('view engine', 'ejs');
app.use(express.urlencoded({ extended: false }));

// @feature list
app.get('/', async (req, res) => {
  res.render('index', { title: 'Items', items: await items() });
});

// @feature api
app.get('/api/items', async (req, res) => {
  res.json(await items());
});

// @feature form
app.get('/contact', (req, res) => {
  res.render('contact', { title: 'Contact', name: '', email: '', errors: {} });
});

app.post('/contact', (req, res) => {
  const { name, email } = req.body;
  if (typeof name != 'string' || typeof email != 'string') return res.status(400).send('missing form field');
  const errors = {};
  const n = [...name].length;
  if (n < 1) errors.name = 'must have at least 1 character';
  if (n > 50) errors.name = 'must have at most 50 characters';
  if (!/^[a-zA-Z0-9.!#$%&'*+\/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$/.test(email)) errors.email = 'must be an email address';
  if (Object.keys(errors).length) {
    return res.status(422).render('contact', { title: 'Contact', name, email, errors });
  }
  console.log(`${name} <${email}>`);
  res.redirect(303, '/');
});

// @feature search
app.get('/search', async (req, res) => {
  res.render('search', { title: 'Search', items: await items() });
});

// @feature setup
app.listen(3000, () => console.log('Listening on http://localhost:3000'));
