// Starts `wisp lsp` for .wisp files; `Wisp: Restart server` starts it again.
const { commands, workspace, window } = require('vscode')
const { LanguageClient } = require('vscode-languageclient/node')

let client

async function start() {
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

async function stop() {
  try {
    await client?.stop()
  } catch {}
  client = undefined
}

exports.activate = async (context) => {
  context.subscriptions.push(
    commands.registerCommand('wisp.restart', async () => {
      await stop()
      await start()
    })
  )
  await start()
}

exports.deactivate = stop
