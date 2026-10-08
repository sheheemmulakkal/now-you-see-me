# Security policy

## Reporting a vulnerability

Please report security problems privately through GitHub: open the
repository's **Security** tab and choose **Report a vulnerability**. Do not
open a public issue for a vulnerability.

Include what you found, how to reproduce it, the affected version
(`nysm --version`) and your Linux distribution. You will get an
acknowledgement within a week, and a fix or a plan once the problem is
understood. Credit is given in the release notes unless you prefer not.

## Supported versions

Only the latest release receives fixes while the project is at 0.x.

## Scope

Now You See Me runs as a normal user, opens no network listeners (the
optional collector uses a private Unix socket) and sends nothing anywhere
unless you ask (`nysm net check`, `nysm tui --remote` over your own SSH).
Of particular interest:

- escaping of untrusted text (process names, paths, interface names) in the
  terminal and desktop views;
- the collector socket: permissions, peer checks, malformed frames;
- recordings and incident files (mode 0600, size limits, parsing);
- the opt-in container-name request to the Docker/Podman socket.

What is read, stored and never sent is described in
[docs/security.md](docs/security.md).
