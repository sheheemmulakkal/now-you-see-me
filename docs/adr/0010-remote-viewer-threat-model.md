# ADR 0010: Remote viewer — design constraints and threat model

Status: proposed (Milestone 5). Step 1 (SSH channel, no listener) is
implemented: `nysm tui --remote` (docs/remote.md). No network listener may
be added until this ADR is accepted and its mitigations are built and
tested.

## Goal
View one's own computer's metrics from a phone/tablet/another machine.
Read-only. No process control.

## Assets
Metrics and process names/PIDs (can reveal what someone works on);
hostnames, interface names, mount points; incident files; alert history.
Integrity of displayed data. The monitored machine itself (a listener is
an attack surface).

## Threat actors
Others on the same LAN/Wi-Fi; internet attackers if exposed; malicious
local users on a shared machine; a stolen/lost paired phone; a compromised
relay (if one is used).

## Threats (STRIDE) and required mitigations
| threat | mitigation |
| --- | --- |
| Spoofing: attacker pretends to be the computer or the phone | mutual authentication with per-device keys exchanged at pairing (QR code containing the host public key + one-time secret); no passwords |
| Tampering / information disclosure on the network | end-to-end encryption (Noise IK or TLS 1.3 with pinned keys); no plaintext HTTP/WebSocket, ever |
| Repudiation | append-only local log of pairings and connections (no metric content) |
| DoS against the monitored host | listener off by default; rate limits; bounded frames (reuse IPC limits); collector never blocks on remote clients (same coalescing as local IPC) |
| Elevation / lateral movement | listener runs as the user, binds only where configured, exposes the read-only snapshot protocol only; no commands except subscribe; no file access; no exec |
| Over-sharing | remote subscription defaults to **lite** (totals only); process names opt-in per paired device; command lines never sent (already true for local IPC) |
| Lost phone | per-device revocation (`nysm remote devices revoke`); short-lived session keys |
| LAN discovery leaks | no mDNS advertisement by default |

## Preferred architecture
1. **SSH first** (already works): run the TUI on the host. Next step that
   needs no new attack surface: `nysm tui --remote user@host` that runs
   `nysm service`'s protocol over an SSH channel (`ssh host nysm service
   stdio`), inheriting SSH host-key verification and credentials.
2. Phone/browser access only after (1), via an explicit opt-in listener
   implementing the mitigations above, or via a user-controlled VPN
   (WireGuard/Tailscale) with the listener bound to that interface only.

## Non-goals
Unauthenticated dashboards; cloud accounts; remote process control;
exposing the local Unix-socket protocol directly on a TCP port.

## Revisit when
A concrete client (phone app or browser) is funded; an external security
review is possible before release.
