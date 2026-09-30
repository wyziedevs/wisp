# Caprese, Nim's entry in the-benchmarker's top ten. `NOSSL=1 nimble install
# -y --depsOnly`, then `nim c -d:release ... server.nim`.
version = "0.1.0"
author = "zenywallet"
description = "Caprese Implementation"
license = "MIT"

requires "caprese >= 0.1 & < 0.2"
