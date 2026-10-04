// @feature layout
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter, Routes, Route, Link } from 'react-router'
import Items from './pages/Items.jsx'
import Contact from './pages/Contact.jsx'
import Search from './pages/Search.jsx'

function Layout({ children }) {
  return (
    <>
      <nav>
        <Link to="/">Items</Link>
        <Link to="/contact">Contact</Link>
        <Link to="/search">Search</Link>
      </nav>
      {children}
    </>
  )
}

createRoot(document.getElementById('root')).render(
  <StrictMode>
    <BrowserRouter>
      <Layout>
        <Routes>
          <Route path="/" element={<Items />} />
          <Route path="/contact" element={<Contact />} />
          <Route path="/search" element={<Search />} />
        </Routes>
      </Layout>
    </BrowserRouter>
  </StrictMode>,
)
