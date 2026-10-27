# Connect a local agent

Open **Agents** and copy the setup prompt, MCP JSON or Codex TOML into your local
client's private configuration. The normal application bundle includes the
companion. Keep its installation path stable and refresh the client's MCP
connection after setup. OpenRadView copies settings only when requested; it does
not edit your agent settings, launch a provider or ask for a provider key.

The reusable configuration runs `openradview-mcp connect` over stdio. It contains
no study-access token. Codex uses `[mcp_servers.openradview]`; compatible JSON
clients use `mcpServers.OpenRadView`. Adapt the wrapper/location to your client's
format. A cloud-only client cannot reach your computer's loopback interface.

For development, build the sibling companion with
`cargo build -p orv-mcp --bin openradview-mcp` and run the viewer with `--features mcp`.
Normal desktop packages already contain both.

## Approve a session

1. Open a study and select a series. Ask the agent to call `list_instances`.
   With several running apps, choose one and pass its `instance_id` to
   `request_access`, together with a recognizable `recipient` label.
2. `request_access` returns `pending`. Review **Allow agent access?** in the app:
   recipient, selected-series scope, image preview, tools and sharing risks.
   Names are self-reported, not verified accounts.
3. Supported tools and selected-series exploration are preselected preferences.
   Expand **Advanced permissions** to restrict them or require exact reviewed
   views. No access exists until **Allow for this session**. X, Escape and
   **Keep private** deny a new request. Closing an existing session's review
   leaves that session active; use **Revoke access** to end it.
4. After responding, call `request_access` again to collect the outcome. An approved
   connector can call `open_study`, `list_series` and then `render_snapshot`.
   Do not repeatedly poll a pending request or retry a denial without instruction.

Only the selected series is accessible. Revoke, starting another import, changing
or closing the series/study, or quitting ends access and cancels outstanding work.
Restart restores no grant. Exact-view grants require another **Review shared view**
approval before a changed scene can be read. A new requester can replace a live
grant only after another explicit Allow. Frozen comparisons reject viewer requests.

The review shows a ready image-only preview and the exact scope. Identifying tags
are omitted, but names may be burned into pixels or inferred from the image.
Your client/provider controls onward sharing, retention, deletion and training.
Check its actual account settings and terms before Allow. OpenRadView cannot
inspect or enforce those controls or recall copies already received. Replies
are untrusted experimental AI commentary, not diagnosis or treatment.

## Tools

| Tool | Purpose |
| --- | --- |
| `list_instances` | App IDs, versions, process IDs and approval ports; no study names, paths or session tokens |
| `request_access` | Ask for desktop approval; call again after the user responds |
| `open_study` | Approved series description and complete current scene; never opens a path |
| `list_series` | The sole approved series, not the whole study |
| `render_snapshot` | Image-only PNG with reproducible scene, geometry, typed source intensity context and hashes |
| `set_view` | Supported window/level, invert, pan/zoom, image, MPR, oblique or projection settings |
| `get_tags` | Allowlisted geometry, not raw identifying tags |
| `measure` / `roi_stats` | Research plane measurements/scalar statistics with valid geometry; no projection measurement |

All seven data tools deny access before approval. For `set_view`, start from the
complete SceneState returned by `open_study`; retain source identity and change
only supported settings. Scalar window/level does not apply to RGB; arbitrary
color LUTs are unavailable. Geometry accompanies PNG manifests independently of
geometry-query permission. ROI statistics cover scalar samples across the approved
plane before PNG windowing/downsampling, including outside a zoomed crop. Desktop
RGB rectangle statistics do not imply RGB support in the MCP `roi_stats` tool.

Snapshots exclude UI annotations and label overlays. They bind source/view/PNG
hashes, dimensions and actual coordinate context. MPR uses the completed rendered
crosshair; oblique records its centre and orthonormal axes; projections record the
source affine, camera, transfer and quality instead of inventing an image plane.
NIfTI scanner/aligned/template references remain distinct. Rendered values may
differ from source pixels. Pending navigation never counts as a completed snapshot.

## Discovery and transports

`openradview-mcp instances --json` lists local instances. The OS chooses available
IPv4 loopback approval ports; no port scan, remote service or fixed port is needed.
The connector stays bound during a request/session and checks the app's identity
before posting. Select explicitly when several instances run.

Discovery files contain only a random app-instance ID, version, PID and port:

- macOS: `~/Library/Caches/OpenRadView/mcp-instances`
- Linux: `$XDG_CACHE_HOME/OpenRadView/mcp-instances`, or `~/.cache/...`
- Windows: `%LOCALAPPDATA%/OpenRadView/mcp-instances`

Unix folders/files use 0700/0600 with matching ownership; Windows inherits user
profile permissions. A live OS lock excludes stale records. Normal shutdown
removes them; a crash may leave nonsecret metadata. Discovery identifies an app
process, not an authenticated agent or a data-sharing grant.

The approval protocol is POST then GET at `http://127.0.0.1:PORT/request`, with
JSON `{"recipient":"Your local agent"}` on POST and an in-memory random
64-lowercase-hex requester bearer capability. Only its holder can collect an
approved endpoint/token once. Approval responses contain `pending`, `denied`, or
`approved`; the connector keeps session tokens out of tool output and settings.
The broker cannot read studies. Exact Host/Authorization checks, rejected Origin
headers, a 4096-byte body limit, one pending request, a three-second new-request
interval, four concurrent requests, five-second handling deadlines and a
180-second lifetime apply. No CORS, proxies or redirects are used.

After Allow, data uses authenticated Streamable HTTP at
`http://127.0.0.1:PORT/mcp`. The port and token are temporary. Turn **Allow agents
to request a session** off under Advanced to stop the approval listener and revoke
sharing. Advanced `connect --broker-url .../request` explicitly overrides discovery.
`stdio --url .../mcp` needs `OPENRADVIEW_TOKEN` from an already approved session;
it cannot create a grant. Manually reviewed direct-session configs contain tokens:
keep them in private settings and remove them when done.

## Compare commentary

**Agents → Advanced → Compare agents using a frozen image…** lets two to four
named local clients receive the same reviewed PNG/manifest with separate tokens.
The two sharing acknowledgements start unchecked. Give each recipient only its
own configuration and copied task. This does not launch clients or contact providers.

Comparison clients have only `comparison_snapshot` and `submit_commentary`.
The final plain-text reply (up to 32768 UTF-8 bytes) includes the exact scene/PNG
hashes and a self-reported model label. Hash matching identifies the requested
context, not authenticated authorship or proof of understanding. Replies remain
untrusted and are displayed without active links or embedded content.

Moving the viewer leaves that comparison frozen. **Revoke and clear comparison**,
source replacement/closure or exit erases local results and revokes tokens. Hiding
the commentary window leaves the visible grant active. Provider/client copies,
logs and clipboard history remain outside OpenRadView's control.

Copied prompts include [shared educational/privacy guidance](../assets/text/agent-guidance.txt).
It asks for supported explanations, calibrated uncertainty and questions for a
clinician, and asks clients not to export/reuse the material. A prompt does not
change provider terms, guarantee no training or validate an answer.
