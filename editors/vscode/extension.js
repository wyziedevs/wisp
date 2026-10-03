// Starts `wisp lsp` for .wisp files.
const { workspace, window } = require('vscode')
const { LanguageClient } = require('vscode-languageclient/node')

let client

exports.activate = async () => {
  const command = workspace.getConfiguration('wisp').get('path') || 'wisp'
  client = new LanguageClient(
    'wisp',
    'Wisp',
    { command, args: ['lsp'] },
    { documentSelector: [{ scheme: 'file', language: 'wisp' }] }
  )
  try {
    await client.start()
  } catch (e) {
    window.showWarningMessage(`Wisp: could not start \`${command} lsp\` (${e.message}). Install the wisp CLI or set wisp.path.`)
  }
}

exports.deactivate = () => client?.stop()
