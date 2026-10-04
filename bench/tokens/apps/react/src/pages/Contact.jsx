// @feature form
import { useState } from 'react'
import { useNavigate } from 'react-router'

export default function Contact() {
  const [name, setName] = useState('')
  const [email, setEmail] = useState('')
  const [errors, setErrors] = useState({})
  const navigate = useNavigate()

  async function handleSubmit(e) {
    e.preventDefault()
    const res = await fetch('/api/contact', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name, email }),
    })
    if (res.ok) navigate('/')
    else setErrors((await res.json()).errors)
  }

  return (
    <form onSubmit={handleSubmit}>
      <title>Contact</title>
      <label>Name <input value={name} onChange={(e) => setName(e.target.value)} required minLength={1} maxLength={50} />
        {errors.name && <small className="problem">{errors.name}</small>}</label>
      <label>Email <input type="email" value={email} onChange={(e) => setEmail(e.target.value)} required />
        {errors.email && <small className="problem">{errors.email}</small>}</label>
      <button>Send</button>
    </form>
  )
}
