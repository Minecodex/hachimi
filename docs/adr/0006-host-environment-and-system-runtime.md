# ADR-0006: Host environment and system runtime discovery

- Status: Accepted
- Date: 2026-09-06
- Owners: Desktop / Runtime / Workspace

## Context

Desktop applications do not necessarily inherit the environment of an interactive terminal. In
particular, a macOS app launched from Finder commonly cannot see Homebrew in `PATH`. Treating a
single fixed location or a Git version floor as installation truth caused a valid Apple Git 2.39.5
to be rejected and then misreported as a Sandbox ACL failure during an initial commit.

Git and the default shell were also being discovered independently by Desktop, Workbench,
Sandbox, and Workspace Worker. That allowed paths and diagnostics to diverge and made a runtime
refresh impossible without restarting the app.

## Decision

1. `hachimi-system-runtime` is the process-wide authority for host-environment discovery, system
   tool probing, refresh, and absolute-path leases. Desktop creates and refreshes its
   `SystemRuntimeManager` at startup, then injects the same handle into command, Workbench, Agent,
   Workspace Host, and Process boundaries.
2. The manager holds an in-memory, refreshable `HostEnvironmentSnapshot` and a monotonically
   increasing revision. It retains only the environment fields needed for discovery (`PATH`,
   `PATHEXT`, account shell and home, Windows system root, temporary directory, and locale). It
   never persists or logs the complete process or login-shell environment.
3. On macOS, the account login shell is resolved from the account database and invoked once as a
   controlled login shell in the user's home directory. Standard input is closed, output is capped
   at 1 MiB, execution is limited to ten seconds, and timeout terminates the process group. A failed
   shell snapshot is a diagnostic warning; process environment and well-known candidates remain
   available.
4. On Windows, inherited Explorer/process environment, User/System environment registry values,
   and registered `App Paths` participate in discovery. PowerShell user profiles are never run at
   application startup. WSL Git is not a Windows-native Worker candidate.
5. All search paths are absolute, normalized, and deduplicated. Empty, relative, current-directory,
   and project/repository-derived paths are rejected. Candidate order is test-only absolute
   override, host shell/Windows environment `PATH`, process `PATH`, and platform well-known
   locations. A failed candidate does not prevent later candidates from being tested.
6. Git acceptance is capability-based rather than version-based. Probes run in an isolated
   temporary repository with system/global configuration, hooks, credentials, prompts, and
   optional locks disabled. Inspect, local mutation, and worktree capabilities are recorded
   independently. Version remains diagnostic metadata; Apple Git 2.39.5 is supported when its
   probes pass. On macOS, `/usr/bin/git` is an Apple shim, so discovery resolves it through the
   system `xcrun` selection to the concrete Command Line Tools/Xcode Git binary before the lease is
   created. Seatbelt derives a read/execute-only helper root from that verified binary so Git's
   adjacent subcommands and templates remain usable without granting project-derived tool paths.
7. A Git lease binds the canonical executable path, file identity, capabilities, and runtime
   revision. Before a new Worker starts the identity is rechecked. Disappearance or replacement is
   `system_git_changed`; a running operation is never silently upgraded to a different Git after a
   refresh.
8. Sandbox is not a tool resolver. Git metadata and mutation preparation receive the leased Git
   path explicitly. Git is removed from `InternalResources`; `RuntimeComponentId::SystemTools` and
   Sandbox readiness are independent health axes. The macOS Sandbox policy marker is advanced to
   `hachimi-macos-seatbelt-v2` and the Windows policy marker to
   `hachimi-windows-appcontainer-v4`; older markers are regenerated without migration.
9. `get_system_runtime`, `refresh_system_runtime`, and `system-runtime-changed` expose one runtime
   snapshot to Composer, Worktree selection, Terminal, and Host settings. `get_default_shell`
   returns `ShellLaunchSpec` rather than an unstructured argument vector. Disabled actions remain
   visible with the exact runtime diagnostic and a refresh action.
10. Tool failures retain the `system_git_*` namespace. `sandbox_acl_prepare_failed` is reserved for
    an actual ACL/Sandbox preparation failure and must not wrap discovery or capability errors.

## Security boundary

The login shell is used only to recover a sanitized host search path, not to authorize project
content or import arbitrary environment into Workers. Production Git commands use the verified
absolute executable, and the restricted Worker receives that path plus the runtime revision through
an allowlisted environment. Projects, repositories, Git configuration, and the current working
directory never nominate system executables.

Browser discovery reuses the sanitized executable-candidate infrastructure but stays lazy. MCP,
Node, Python, and other project/plugin tools remain activation-scoped. Bundled CEF, model, and
sidecar resources continue to use the resource manifest and hash-attestation path.

## Consequences

- A Finder-launched macOS app can discover Homebrew Git while retaining Apple Git as a valid later
  candidate.
- Installing or repairing Git can be recovered with a runtime refresh; application restart is not
  required.
- Git absence no longer prevents the Desktop or Sandbox runtime from becoming ready. Only features
  requiring a missing capability are disabled.
- There is no legacy resolver facade, immutable Git-path cache, manual Git-path setting, or Git
  2.40 compatibility branch.

## Reference boundary

The behavioral baseline is OpenAI Codex commit
`2cfee7de25d98b20c55ced2ed82736d4c452f8a0`, specifically
`codex-rs/core/src/shell_snapshot.rs`,
`codex-rs/config/src/shell_environment_policy.rs`, and
`codex-rs/cli/src/doctor/git.rs`. The public configuration reference documents shell snapshots as
enabled by default and environment inheritance/filtering as a separate policy.

Hachimi's minimal in-memory snapshot, Windows registry/App Paths merge, capability matrix,
absolute-path file-identity lease, revision fencing, Tauri protocol, and UI are original
implementations. No Codex source is copied into `hachimi-system-runtime`. This is deliberately
stricter than relying on an ambient `git` command after discovery.

- Source commit: <https://github.com/openai/codex/tree/2cfee7de25d98b20c55ced2ed82736d4c452f8a0>
- Configuration reference: <https://learn.chatgpt.com/zh-Hans/docs/config-file/config-reference>

## Acceptance

Unit coverage includes sanitized Finder-style paths, Homebrew and Apple Git, Windows registry and
standard locations, probe failures/timeouts, independent command/PTY shell capabilities, partial
Git capability results, refresh revisions, and executable drift. Integration coverage binds one
absolute Git lease across Workbench, ACL metadata, and Worker launch. The packaged macOS smoke runs
both normal discovery and the Apple `/usr/bin/git` route through the actual `.app` resource Worker
and Seatbelt, then proves an empty initial commit leaves the index and untracked files unchanged.
Desktop E2E covers Git-unavailable disabled state and recovery after refresh. Windows CI creates a
disposable standard user, clears its process `PATH` to system directories, and requires registry or
well-known Git for Windows discovery to succeed.
