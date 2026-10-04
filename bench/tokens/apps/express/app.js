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
const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

app.get('/contact', (req, res) => {
  res.render('contact', { title: 'Contact', name: '', email: '', errors: {} });
});

app.post('/contact', (req, res) => {
  const name = String(req.body.name ?? '');
  const email = String(req.body.email ?? '');
  const errors = {};
  if (name.length < 1 || name.length > 50) errors.name = 'Name must be 1 to 50 characters';
  if (!EMAIL.test(email)) errors.email = 'Enter a valid email';
  if (errors.name || errors.email) {
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
