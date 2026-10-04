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
app.post('/api/contact', (req, res) => {
  const { name, email } = req.body
  const errors = {}
  if (!name || name.length > 50) errors.name = 'must have 1 to 50 characters'
  if (!/^\S+@\S+\.\S+$/.test(email)) errors.email = 'must be an email address'
  if (Object.keys(errors).length) return res.status(422).json({ errors })
  console.log(`${name} <${email}>`)
  res.json({ ok: true })
})

// @feature setup
app.listen(3000, () => console.log('API on http://localhost:3000'))
