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
  if (typeof name != 'string' || typeof email != 'string') return res.status(400).send('missing form field')
  const errors = {}
  const n = [...name].length
  if (n < 1) errors.name = 'must have at least 1 character'
  if (n > 50) errors.name = 'must have at most 50 characters'
  if (!/^[a-zA-Z0-9.!#$%&'*+\/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$/.test(email)) errors.email = 'must be an email address'
  if (Object.keys(errors).length) return res.status(422).json({ errors })
  console.log(`${name} <${email}>`)
  res.json({ ok: true })
})

// @feature setup
app.listen(3000, () => console.log('API on http://localhost:3000'))
