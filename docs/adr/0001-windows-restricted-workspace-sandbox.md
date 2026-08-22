# ADR-0001: Windows restricted Workspace Sandbox

- Status: Accepted
- Date: 2026-07-25
- Owners: Harness Agent / Desktop

## Context

The Workspace Worker is already a separate process, but it currently inherits the desktop user's
Windows access token. Process separation alone does not enforce the persisted filesystem, process,
or network grants. Hachimi must not report an enforced sandbox until a runtime probe proves the OS
boundary.

## Decision

1. Windows is the first enforced backend. `ReadOnly` may run with an explicit degraded report;
   workspace writes and process execution fail closed unless the backend is attested.
2. NSIS uses per-user installation. On first launch Hachimi atomically stages fixed-hash sidecars
   under the per-user data root, then the current user runs the setup helper
   without `runas`, UAC, a Windows Service, or system Git changes.
3. The marker is not an attestation. Startup resolves the installed AppContainer identity and SID,
   verifies the policy version, creates an AppContainer process security context, and runs
   filesystem/process/network canaries before setting `SandboxStatus::Enforced`.
4. Every side-effecting Workspace Worker is launched through `SandboxBackend::spawn_restricted`.
   The child and all descendants are assigned to a kill-on-close Job Object, inherit only explicit
   stdio handles, receive an allowlisted environment, and use a Run-scoped temporary directory.
   Windows process creation registers both `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` and
   `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`; the latter contains only child-side stdin/stdout/stderr
   handles created for that launch. With no stdio, handle inheritance is disabled. An inheritable
   sentinel handle that is deliberately omitted from the list must remain unusable in the child.
5. C2.1 enforces deny-all network access. Host/protocol allow rules remain a separate C2.2 change.
6. Path authorization uses Windows-native normalization plus final-handle verification. UNC,
   device namespaces, alternate data streams, escaping reparse points, and unsupported volumes are
   rejected for the first release.
7. `SandboxRuntimeManager` is the live readiness authority. Workbench can refresh, attest or repair;
   repair first restages the packaged Runtime, immediately narrows all enforcement flags, reruns the
   per-user helper, then proves sidecar SHA-256 and all canaries. Workspace mutation,
   Process spawn, stdio MCP and broker startup are mutually exclusive with repair.
8. A Run keeps its creation-time Sandbox snapshot, while every side effect intersects it with the
   current runtime report. Repair can narrow an active Run but cannot upgrade it.
9. Checkout roots must be current-user-owned local NTFS directories outside protected Windows
   roots. Ownership mismatch, protected roots, reparse points and unsafe ACL application return
   stable migration diagnostics; repair never broadens access automatically.

## Consequences

- The Agent kernel still receives no filesystem or process handle.
- `SandboxCapabilityReport` is the only UI source of readiness truth.
- A setup or canary failure disables write/Exec instead of silently falling back to the desktop
  user token.
- The required release path is a standard Windows user clean install/repair/upgrade. Administrator
  smoke and control of elevated applications are separate later validation and do not block this ADR.

## Source boundary

The filesystem, process-tree, setup, attestation, and fail-closed boundaries were reviewed against
OpenAI Codex at fixed commit `4c43465133428898aa84f0bfc02c306ed65fb66a` using fixed-object
inspection. Hachimi's AppContainer identity, launch plumbing, installer integration, and runtime
canaries are original implementations. The refreshable setup orchestration in
`crates/hachimi-sandbox/src/runtime_manager.rs` selectively adapts Codex setup-orchestrator control
flow and is registered per file in the provenance ledger; it does not embed Codex Core or reuse
Codex product prompts.

## Amendment 2026-08-22: pinned MinGit removed in favor of system Git

The per-user Runtime no longer stages a pinned MinGit distribution, and the managed-Git SHA-256
manifest attestation is removed. Git operations resolve the system Git installation (>= 2.40) from
fixed well-known locations before PATH, matching openai/codex `codex-rs/git-utils` @
`4f39251a010a8bd7d692d25fb33832ff06f1635a`. The contract that restricted Workers receive an explicit
absolute Git path and never resolve Git from their checkout directory is unchanged, as is the
config-level hardening (disabled `core.hooksPath`, `GIT_OPTIONAL_LOCKS=0`, allowlisted environment).
Rationale: the pinned binary and hash manifest only defended against tampering that requires
administrator rights, and an administrator already owns the machine; the lookup-attack vector is
closed by explicit-path injection. This also unblocks the macOS port (`docs/mac-plan/`), which has
no MinGit equivalent.
