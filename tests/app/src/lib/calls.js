import { add } from 'wisp:remote'

// Calls a #[remote] function from a lib file, by its import.
export const addUp = (a, b) => add(a, b)
