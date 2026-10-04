// @feature form
const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/

export default defineEventHandler(async (event) => {
  const body = await readBody(event)
  const name = String(body.name ?? '')
  const email = String(body.email ?? '')
  const errors = {}
  if (name.length < 1 || name.length > 50) errors.name = 'Name must be 1 to 50 characters'
  if (!EMAIL.test(email)) errors.email = 'Enter a valid email'
  if (errors.name || errors.email) {
    throw createError({ statusCode: 422, data: { errors } })
  }
  console.log(`${name} <${email}>`)
  return { ok: true }
})
