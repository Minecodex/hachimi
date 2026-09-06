# ADR 0004: Local Hosts and extension boundary

- Status: Accepted
- Date: 2026-07-28
- Amended: 2026-07-30

## Context

Browser, desktop control, Plugins/Connectors and external Channels must extend the product without
creating a second Agent loop or allowing untrusted content to manufacture authority.

## Decision

1. `browser.*`, `computer.*`, `plugin.*`, `connector.*`, `channel.*` and `gateway.*` are typed
   AppServer domains. Model-visible tools still execute through the single `AgentRunExecutor`,
   StepContext, Policy, Approval, Capability Grant and Sandbox chain.
2. Browser supports a managed Chromium isolated Profile and an explicitly paired Chrome extension.
   Origin/capability grants, task-owned tabs and observation IDs fence every action. Upload/download
   use short-lived Session-bound tokens and isolated storage.
3. Computer uses Windows Graphics Capture for Observe and allowlisted `SendInput` for Act. Every
   action binds a Frame, App and Window fingerprint. Elevated/protected desktops, Hachimi windows,
   background windows and stale Frames fail closed.
4. Plugin bundles are local, content-addressed and manifest-bounded. Lifecycle changes and upgrades
   never silently expand permissions. Connector credentials live in Windows Credential Manager;
   SQLite stores only `secret_ref`, revision and metadata.
5. `sample-crm` is the deterministic Connector acceptance fixture. Plugin distribution remains
   local and content-addressed; the complete contribution update/rollback/reconciliation lifecycle
   is prepared for implementation.
6. Channel/Gateway only authenticates, routes and persists ingress/outbox. It cannot approve a
   high-risk action and owns no model loop. `loopback-webhook` and `mock-poll` are deterministic
   acceptance fixtures. Production Connector/Channel work is limited to WeCom, DingTalk and Feishu.
7. Page text, DOM, downloaded content, Connector data and Channel messages are untrusted external
   content. They cannot alter system instructions, grants, approvals or pinned revisions.

## Consequences

- Browser screenshots and Computer frames may enter only the current in-memory model request and are
  excluded from Transcript, SQLite, Audit and durable side-effect results.
- Plugin/Connector and Channel/Gateway status is accurately described as “framework and samples
  complete; full contribution lifecycle and WeCom/DingTalk/Feishu integrations prepared.”
- Local Host kill switches can reduce availability but cannot create authority.

## Source boundary

Codex public product behavior is the primary permission/interaction reference. OpenClaw fixed commit
`f6d456235cf011004f7cffc71a95acf6fbf1fa0a` is a behavior reference for Channel routing and durable
delivery. Current Local Host implementations are original Hachimi code; exact derivations, if any,
must be registered before adaptation.

## Amendment 2026-08-23: macOS Computer host

The Computer host gains a macOS broker (`crates/hachimi-computer/src/platform/macos.rs`):
enumeration/identity via `CGWindowList` + `NSRunningApplication`, one-shot capture via
`SCScreenshotManager` (macOS 14+), input injection via `CGEvent` (US-ANSI virtual keycodes, Unicode
text payloads, line-unit scroll), and window operations via AXUIElement (position/size/minimize/
zoom/close) plus `NSRunningApplication.activateWithOptions`. Elevated targets map to root-owned
processes (`proc_pidinfo`); there is no protected-desktop equivalent on macOS, and TCC permission
state (Screen Recording / Accessibility) replaces it as the fail-closed gate with stable error codes
`computer_capture_requires_macos14`, `computer_screen_recording_required` and
`computer_accessibility_required`. User-takeover fencing maps the Windows foreground-window gate to
frontmost-application matching. `LaunchApp` resolves bundle identifiers, app names and `.app` paths
through `/usr/bin/open -a`; the protocol's `.exe` shape validation is now platform-split.

## Amendment 2026-09-05: macOS Browser host

The Browser embedded host gains a macOS implementation (`crates/hachimi-cef-host` plus
`apps/desktop/src-tauri/src/cef_overlay.rs`). macOS has no cross-process NSView embedding equivalent
to the Win32 child-HWND model, so the managed Chromium profile renders windowless (OSR) and the
desktop composites frames above the WKWebView through a per-tab native overlay view; input routes
the opposite way through a typed `SendInput` IPC command onto `BrowserHost::send_*_event`,
preserving the existing UserInput epoch and shortcut fencing. Origin grants, task-owned tabs,
observation IDs and upload/download tokens are unchanged. CEF renderer/GPU/helper processes run
under Chromium's built-in Seatbelt sandbox (helpers call `cef_sandbox_initialize` before the
framework loads); this is independent of, and does not conflict with, the workspace Seatbelt backend
(ADR 0001). Distribution currently targets a dev channel: an ad-hoc-signed dmg via GitHub Release;
Developer ID signing plus notarization remains the release gate (docs/mac-plan/phase-5).
