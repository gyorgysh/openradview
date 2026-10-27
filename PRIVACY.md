# How OpenRadView handles data

OpenRadView reads your images on your device. The desktop app has no telemetry,
analytics, advertising, app account, hosted scan service, remote fonts or embedded
web content. Maintainers do not receive your scans through the viewer.

## Local viewing and storage

Images are opened when you select or drop local files. The study index, bounded
decoded cache, series previews, measurements and view state are held in memory. Requested frames
are read from the selected sources on background workers, so keep those files
available while viewing. ZIP data streams without extraction or a patient-data
disk cache. No recent-study list or patient database is saved.

Closing a study releases app-owned references; it does not guarantee forensic
memory erasure. Operating-system swap, file-dialog history, backups and screen
capture are controlled by your device. The research-use notice is per running
session and is not transmitted. Local labels remain in memory until you export them.

The app saves update preferences, the last check time and installation/backup
state. These contain no study data. A staged update and previous app copy may
remain until installation and the new app's startup complete.

## GitHub updates

In standalone installations, automatic GitHub update checks are enabled by
default. Clear the checkbox on first entry or turn them off later under **Privacy → Updates**. Manual checks
are available there too. Enabled background checks run about once a day; a manual
check contacts GitHub when requested. Download, installation and restart are
separate user actions, not automatic consequences of a check.

Debian package installations disable automatic checks and in-app replacement.
You can check manually, then download and install a newer package yourself.
The app does not configure an apt repository.

Update requests contain your IP address, app version in the user agent,
platform/architecture in the requested asset name and ordinary HTTPS/network
information. They contain no images, tags, filenames, study identifiers or usage
events. System profiling is disabled. GitHub and its download infrastructure
process those requests under their own privacy policies. No GitHub password or
access token is stored in the app. Failed checks do not grant access to scans.
macOS signature/Gatekeeper assessment may contact Apple's operating-system trust
services.

## Optional agent sessions

The normal bundle includes a local MCP connector. After entering the workspace,
an approval-only listener uses an available `127.0.0.1` port. It cannot read study
data. It rejects browser origins, bounds requests and expires them after three
minutes. Turn requests off in **Agents → Advanced** to stop it and revoke sharing.

The connector discovers app instances through private per-user cache files
containing only a random instance ID, app version, process ID and approval port.
There are no study names, file paths or tokens in them. A live OS lock excludes
stale records. Normal shutdown removes them; a crash may leave nonsecret metadata.
[Locations and protocol](docs/agent-access.md#discovery-and-transports)

Each data-sharing session requires a separate review showing the recipient,
selected series, image preview, tools and onward-sharing risks. Supported tools
and series exploration are preselected preferences; nothing is granted until
**Allow**. Advanced permissions can restrict tools or require exact reviewed views.
Closing a new request denies it. An approved endpoint and bearer token exist only
in memory for that session. Client names/accounts are self-reported and unverified;
anyone holding the approved token can use its allowed tools.

The selected-series grant excludes other series and arbitrary file paths. Metadata
uses a fixed allowlist of geometry, image context and content/view hashes. Raw
patient names, dates, UIDs, descriptions and paths are omitted. PNGs exclude UI
annotations and label overlays and include a reproducible scene/geometry manifest.
Enabled ROI queries can cover approved samples outside the displayed zoomed crop.
These measures do not guarantee anonymity: identifying text can be burned into
pixels, and information can be inferred from the image.

The viewer does not contact an agent provider. Your configured client may forward
approved data off the device. Its provider/account controls logs, storage,
retention, training, deletion and onward transfers. Check the actual terms and
settings before Allow. OpenRadView cannot inspect or enforce those controls,
verify the provider account or recall delivered copies. Copied guidance asks
clients not to export or reuse the material; a prompt cannot change provider
policy. Replies are untrusted experimental AI commentary and may be wrong.

Revoke, a new import, a series change, study closure or quitting invalidates the
grant and cancels outstanding work. Restart restores no grant. Sources are
fingerprinted before review and verified again when read after approval; changed
source content is refused. Revocation prevents future access, not copies already
received by a client.

Optional comparisons share one frozen reviewed image/context with separately
named recipients and temporary tokens. Both sharing acknowledgements start
unchecked. Replies stay in memory as bounded plain text. Revoke/clear, source
replacement/closure or exit erases local results and revokes the tokens. Hiding
the commentary window leaves the visible session active.

## Exports, clipboard and external services

Picture and label exports happen only when requested and remain under your
control. Original files are not overwritten. Exported pixels can still contain
sensitive content; review them before sharing. No automatic exports are made.

The app copies connector settings only when you press Copy. Reusable settings
contain no session token; advanced direct-session settings do. Clipboard managers
and agent settings may retain copies outside the app's control. OpenRadView does
not automatically write agent credentials or connection settings to disk.

Website visits, GitHub downloads, sponsorship and support channels have their own
privacy notices and may receive IP addresses/access logs without analytics. Do
not put scans, health information or session tokens in issues, support messages
or screenshots. Development tools/CI fetch dependencies and communicate with
the source host; that is separate from installed-app viewing.

No GDPR, HIPAA or worldwide privacy certification is claimed. A session permission
controls access in this app; it does not establish another service's legal basis,
provider contract or transfer safeguards. See the [disclaimer](DISCLAIMER.md)
for the app's intended use.
