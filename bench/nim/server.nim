# Caprese: its the-benchmarker entry (nim/caprese), first on their board,
# answering only their routes. Its port is fixed at build (3000), and it
# starts a thread per CPU the machine has (Linux only).
import caprese

config:
  sslLib = None
  headerServer = true
  headerDate = true
  headerContentType = true
  activeHeader = true
  connectionPreferred = InternalConnection
  postRequestMethod = true

server(ip = "0.0.0.0", port = 3000):
  routes:
    get "/": "".addHeader("text").send
    get "/user/:id": id.addHeader("text").send
    post "/user": "".addHeader("text").send

serverStart()
