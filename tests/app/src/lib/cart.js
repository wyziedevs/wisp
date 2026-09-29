// A store shared by every page that imports it; it outlives navigations.
import { store, persisted, derived } from 'wisp'
import { label } from './names.js'

export const cart = store([])
export const theme = persisted('a2-theme', 'light')
export const size = derived(() => cart.value.length)
export const title = label('cart')
