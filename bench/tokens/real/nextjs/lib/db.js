// @feature data
export const users = [];
export const sessions = new Map();
export const posts = new Map();
let lastId = 0;
export const nextId = () => ++lastId;

// @feature crud
export function checkPost({ title, body }) {
  const errors = {};
  if (!title || title.length > 100) errors.title = 'Title must be 1 to 100 characters';
  if (!body) errors.body = 'Body is required';
  return errors.title || errors.body ? errors : null;
}

// @feature live
export const listeners = new Set();
