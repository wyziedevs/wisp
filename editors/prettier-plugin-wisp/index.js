// Prettier formats .wisp files with `wisp fmt --stdin`, the same layout as
// `wisp fmt`. Without the wisp command the text stays as it is, with one
// warning.
import { execFileSync } from 'node:child_process'

let warned = false

function wispFmt(text, options) {
  const args = ['fmt', '--stdin']
  if (options.filepath) args.push(options.filepath)
  try {
    return execFileSync(options.wispPath || 'wisp', args, {
      input: text,
      encoding: 'utf8',
      stdio: ['pipe', 'pipe', 'pipe'],
      maxBuffer: 1 << 28,
      windowsHide: true,
    })
  } catch (e) {
    if (!warned) {
      warned = true
      const why = e.code === 'ENOENT' ? 'it is not installed or not on PATH' : (e.stderr || e.message).trim()
      console.warn(`prettier-plugin-wisp: \`${options.wispPath || 'wisp'} fmt\` failed (${why}); .wisp files are left as they are.`)
    }
    return text
  }
}

export const languages = [
  { name: 'Wisp', parsers: ['wisp'], extensions: ['.wisp'], vscodeLanguageIds: ['wisp'] },
]

export const options = {
  wispPath: {
    type: 'string',
    category: 'Wisp',
    default: 'wisp',
    description: 'The wisp command, which runs `wisp fmt --stdin`.',
  },
}

export const parsers = {
  wisp: {
    astFormat: 'wisp',
    parse: (text, options) => ({ text: wispFmt(text, options) }),
    locStart: () => 0,
    locEnd: (node) => node.text.length,
  },
}

export const printers = {
  wisp: { print: (path) => path.node.text },
}
