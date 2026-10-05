// @feature form
export default defineEventHandler(async (event) => {
  const { name, email } = await readBody(event)
  if (typeof name != 'string' || typeof email != 'string') throw createError({ statusCode: 400 })
  const errors = {}
  const n = [...name].length
  if (n < 1) errors.name = 'must have at least 1 character'
  if (n > 50) errors.name = 'must have at most 50 characters'
  if (!/^[a-zA-Z0-9.!#$%&'*+\/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$/.test(email)) errors.email = 'must be an email address'
  if (Object.keys(errors).length) {
    throw createError({ statusCode: 422, data: { errors } })
  }
  console.log(`${name} <${email}>`)
  return { ok: true }
})
