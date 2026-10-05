# Security

## Reporting a vulnerability

Please do not open a public issue for a security problem. Use GitHub's private reporting: open the [Security tab](https://github.com/wyziedevs/wisp/security/advisories/new) of this repository and choose "Report a vulnerability". Include the version or commit, what you did, and what happened.

## Supported versions

Wisp is pre-1.0. Fixes land on `main` and ship in the next release; only the latest release is supported.

## Scope

The runtime (`wisp-web-rt`), the build step (`wisp-web-build`), the CLI (`wisp-web`) and the code they generate. What the framework does about request limits, CSRF, cookies and the rest is described at https://wispweb.dev/docs/security/.
