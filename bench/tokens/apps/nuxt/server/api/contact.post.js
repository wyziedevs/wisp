// @feature form
export default defineEventHandler(async (event) => {
  const { name, email } = await readBody(event)
  const errors = {}
  if (!name || name.length > 50) errors.name = 'must have 1 to 50 characters'
  if (!/^\S+@\S+\.\S+$/.test(email)) errors.email = 'must be an email address'
  if (Object.keys(errors).length) {
    throw createError({ statusCode: 422, data: { errors } })
  }
  console.log(`${name} <${email}>`)
  return { ok: true }
})
