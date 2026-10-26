# OpenRadView engineering instructions

Read the current documentation and git status before editing. Preserve unrelated
work. Local docs/plan.md, PROGRESS.md, BLOCKERS.md and docs/worklogs/ are gitignored;
use them for plans, investigation, legal notes and resumption. Keep public docs
focused on current behavior, user guidance and necessary contributor/release work.
Do not add a Markdown report or tag for every development task.

## Product and privacy

- Intended use is personal education with the user's own images. No clinical
  validation, diagnosis, treatment or care decisions; seek professional interpretation.
  State intended use rather than an unsupported legal classification.
- No telemetry, accounts, ads, hosted scans, remote fonts or embedded web content.
  Update traffic stays in orv-updates and carries no study data. Save the first-run
  update preference; downloads/installations/restart require user action. Other
  networking stays in optional MCP/agent crates and is local by default.
- Never expose images or metadata without explicit per-session Allow showing the
  recipient, exact scope and preview. The approval-only broker cannot read studies.
  Preserve selected-series/tool/exact-view boundaries, expiry, denial and revoke.
  No tool may accept arbitrary paths or return raw identifying tags.
- Client names are self-reported. Provider retention/training is outside the app's
  control; prompts cannot guarantee it. Removing tags cannot remove burned-in names.
  Use assets/text/agent-guidance.txt and label replies experimental AI commentary.
- No patient data, identifying screenshots/paths, credentials or local logs in git.
  Use generated fixtures or credited licensed demos. Local viewing needs no sharing.

## Implementation

- Rust 2024, exact dependencies and pinned toolchain. Core/render crates never
  depend on UI crates. No unsafe; no panics in normal operation.
- Keep loading, decoding, statistics, filtering and export off the UI thread.
  Preserve cancellation, bounded memory, validity masks and source/view identity.
  Idle views should not poll or repaint continuously.
- Preserve supplied outlined artwork and monochrome palette. Use native UI and
  bundled fonts/assets; honor reduced motion. Check exact pinned library APIs.
  Record durable design decisions in docs/architecture.md, not milestone diaries.
- Retain GPL SPDX headers and upstream license/source/patch inventories. Comments
  should explain constraints or non-obvious decisions, not narrate the task or chat.
- Run fmt, default/all-feature all-target Clippy with warnings denied, workspace
  nextest and build checks appropriate to the change. GPU tests must actually run;
  a skip is not a pass. Visually check native UI changes. See CONTRIBUTING.md.

## Source and releases

- Use DCO sign-offs (`git commit -s`). Version tags identify actual release candidates
  or releases; do not recreate development milestone tags. Changelog entries describe
  changes relevant to readers. Visibility changes and publication need owner instructions.
- Development CI stays manual and cached. Private release jobs require explicit
  owner dispatch; public tag builds require the release environment reviewer gate.
  Do not add routine branch/PR builds or dispatch paid jobs without authorization.
- Credentials belong only in the GitHub release environment, never repository-level
  secrets, source, logs or chat. Key export/upload needs explicit owner authorization.
  See docs/github-release.md. Never silently fall back to unsigned artifacts.
- No public release, visibility change or website deployment is implied by build,
  review or copy work. Preserve exact source, notices, signed manifest, artifact
  hashes, notarization checks and immutable publication gates.
- Signing/notarization, free/GPL distribution and consent do not determine medical
  software qualification. Keep function-specific distributor/hosting/legal questions
  in local notes; do not claim universal compliance or clearance.

## Copy and promotion

Use plain, specific wording and the no-ai-slop skill when available. Describe opening,
viewing and learning from one's own images. Keep necessary intended-use and privacy
information. Do not promise diagnosis, disease detection, reassurance, AI second
opinions, medical confidence, professional endorsement, regulator approval or blanket
privacy/security/performance guarantees. Explain actual limits and platform evidence.

The project is built by gyorgy.sh; pueev OÜ is the planned website publisher and
Apple distributor. Verify actual public hosting/privacy facts before launch.
Sponsorship is voluntary maintenance support and unlocks no features or services;
keep this maintainer policy out of product introductions. GPL permits commercial
redistribution; do not add a noncommercial restriction.

Use the approved native public screenshot with its dataset credits, or generated
fixtures. Do not fabricate anatomy or diagnostic callouts. Preserve finite motion,
static alternatives and reduced motion. Check public copy, metadata and downloads
against the implementation before publication; update only facts that changed.
