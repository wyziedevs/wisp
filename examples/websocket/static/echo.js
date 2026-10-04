const ws = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`)
const log = document.getElementById('log')
const say = document.getElementById('say')
const add = (text) => log.append(Object.assign(document.createElement('li'), { textContent: text }))

ws.onmessage = (e) => add(e.data)
ws.onclose = () => add('closed')
document.getElementById('send').onclick = () => {
  ws.send(say.value)
  say.value = ''
}
