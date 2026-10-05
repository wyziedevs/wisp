#!/bin/bash
# Installs the toolchains the 12 apps need (Ubuntu 24.04, run as root). Go, .NET and the Python venv
# live under /root; Java and Maven come from apt. Rust and Node/Bun are expected already (see README).
set -u
mkdir -p /root/nr-tools
cd /root/nr-tools || exit 1
V=$(curl -fsSL 'https://go.dev/VERSION?m=text' | head -1)
echo "go $V"
[ -x go/bin/go ] || curl -fsSL "https://go.dev/dl/$V.linux-amd64.tar.gz" | tar xz
[ -x /root/dotnet/dotnet ] || { curl -fsSL https://dot.net/v1/dotnet-install.sh -o di.sh; bash di.sh --channel 10.0 --install-dir /root/dotnet; }
export DEBIAN_FRONTEND=noninteractive
apt-get install -y -q openjdk-21-jdk-headless maven python3-venv python3-pip 2>&1 | tail -3
/root/nr-tools/go/bin/go version
/root/dotnet/dotnet --version
java -version 2>&1 | head -1
mvn -v | head -1
python3 -m venv --help > /dev/null && echo "venv ok"
