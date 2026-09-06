# OpenAI Codex macOS Seatbelt sandbox behavior snapshot

Canonical source: https://github.com/openai/codex/tree/4f39251a010a8bd7d692d25fb33832ff06f1635a/codex-rs/sandboxing

Retrieved: 2026-08-23T00:00:00+08:00

Hachimi's macOS sandbox backend uses the following documented implementation behavior from this fixed snapshot:

- Sandbox enforcement uses `/usr/bin/sandbox-exec` with a generated SBPL profile; the fixed absolute path defends against PATH injection.
- The base profile starts `deny default`, allows process-exec/fork and same-sandbox signals, whitelists the sysctls, pseudo-TTY, mach-lookup and IPC primitives a development workload needs, and inlines platform read defaults for system frameworks, `/usr/lib`, and standard special files.
- Filesystem grants become `(subpath (param ...))` allow rules parameterized with `-D`; writable roots get a `file-write-unlink` anchor deny so the authority boundary cannot be replaced from inside.
- Protected metadata (repository `.git`) is carved out of writable roots with regex denies, keeping read-only Git metadata inside writable checkouts.
- Network policy is composed separately; Hachimi's C2.1 uses a plain `(deny network*)`.

Hachimi maps its Run-scoped CapabilityGrantSet onto this profile at each launch and keeps its own one-shot worker protocol, attestation canaries, and fail-closed readiness model. Hachimi does not embed Codex Core or reuse Codex product prompts.
