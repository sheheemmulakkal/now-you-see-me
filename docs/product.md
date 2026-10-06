# Product

## Users
Software, infrastructure and performance engineers on Linux workstations,
servers reached over SSH, VMs/containers, and small ARM boards.

## Core questions (in priority order)
1. What resources are used now?
2. What changed over the last few minutes?
3. Which process/app/service/container is responsible?
4. Is work waiting on a constrained resource (pressure, not just usage)?
5. Did a change improve performance (record/compare)?

## Workflows
- **Quick look**: `nysm` → summary with real rates in ~1 s.
- **Live triage**: `nysm tui` → overview trends + top processes → drill into
  process details; pause to read while collection continues; move the shared
  timeline cursor to see all resources at one moment.
- **Automation**: `nysm watch --format jsonl`, `summary --json`,
  `processes --json`, stable exit codes.
- **Remote**: SSH in and run `nysm tui` there; no agent required.
- **Diagnose the monitor itself**: `nysm doctor`, `nysm capabilities`.

## Non-goals
- No cloud backend, accounts, mandatory telemetry or AI explanations.
- No process control in early releases (observational only); never
  auto-kill.
- No promise of identical semantics across OSes — differences are labelled.
- No unauthenticated network endpoints.

## Feature priorities
1. Correct core metrics with explicit statuses (done for Linux).
2. CLI + TUI (done for M1).
3. Engineer workflows: ports, recordings/compare, alerts, history (M2).
4. Optional service, GTK desktop, GNOME panel (M3).
5. Containers, services, sensors, packaging (M4).
