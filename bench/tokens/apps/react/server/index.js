// @feature setup
import express from 'express'
import { items } from './db.js'

const app = express()
app.use(express.json())

// @feature api
app.get('/api/items', async (req, res) => {
  res.json(await items())
})

// @feature form
const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/

app.post('/api/contact', (req, res) => {
  const name = String(req.body.name ?? '')
  const email = String(req.body.email ?? '')
  const errors = {}
  if (name.length < 1 || name.length > 50) errors.name = 'Name must be 1 to 50 characters'
  if (!EMAIL.test(email)) errors.email = 'Enter a valid email'
  if (errors.name || errors.email) return res.status(422).json({ errors })
  console.log(`${name} <${email}>`)
  res.json({ ok: true })
})

// @feature setup
app.listen(3000, () => console.log('API on http://localhost:3000'))
