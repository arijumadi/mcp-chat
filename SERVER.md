# Running Jarvis on a server

`jarvis-server` is the same engine and the same web UI as the desktop app, with
an HTTP transport instead of the Tauri shell. The workspace, the database, the
MCP servers, the ACP agents, and every tool process stay on the host. The
browser is only a view.

It is not a rewrite: the frontend calls `invoke(command, args)` in both modes.
The desktop uses Tauri IPC; the server answers `POST /api/invoke` with the same
command names and the same argument shapes.

## Getting the binary

Builds are produced by the `Server build` workflow and published as
`jarvis-server-linux-x64.tar.gz`, which contains:

```
jarvis-server
dist/            the web UI bundle
```

There is nothing else to install. No Node, no WebKit, no X server, no database
server. SQLite is bundled into the binary.

## Running it

```bash
JARVIS_DATA_DIR=/var/lib/jarvis \
JARVIS_WORKSPACE=/srv/jarvis/workspace \
JARVIS_STATIC_DIR=/opt/jarvis/dist \
/opt/jarvis/jarvis-server
```

systemd unit:

```ini
[Unit]
Description=Jarvis
After=network-online.target

[Service]
ExecStart=/opt/jarvis/jarvis-server
WorkingDirectory=/srv/jarvis
Environment=JARVIS_DATA_DIR=/var/lib/jarvis
Environment=JARVIS_WORKSPACE=/srv/jarvis/workspace
Environment=JARVIS_STATIC_DIR=/opt/jarvis/dist
Environment=HOME=/var/lib/jarvis
User=jarvis
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

`HOME` must point at a writable directory even though Jarvis itself does not
need it: the ACP agents (`opencode`, `agy`, …) keep their credentials under
`~/.config`, and without a real `HOME` they cannot log in.

### Environment

| Variable | Default | Meaning |
|---|---|---|
| `JARVIS_DATA_DIR` | `$XDG_DATA_HOME/jarvis` | database, attachments, tool output, secrets |
| `JARVIS_WORKSPACE` | `$JARVIS_DATA_DIR/workspace` | default workspace for new chats |
| `JARVIS_STATIC_DIR` | `dist/` beside the binary | the web UI bundle |
| `JARVIS_BIND` | `127.0.0.1:8787` | listen address |
| `JARVIS_TOKEN` | generated once into `server-token` | the access token |
| `JARVIS_SECURE_COOKIE` | `0` | set to `1` when serving over HTTPS |
| `JARVIS_ALLOWED_HOST` | unset | pins the `Host` header against DNS rebinding |

On first start the server prints a token. Open `http://<host>:8787/login/`,
paste it, and the browser keeps an HttpOnly cookie.

## Reaching it from another computer

**Do not expose this port to the internet without TLS and a reverse proxy.** It
runs `bash` and writes files as its user; the token is the only thing between a
request and a shell.

Recommended, in order of least risk:

1. **SSH tunnel** — nothing listens on the network at all:
   ```bash
   ssh -N -L 8787:127.0.0.1:8787 you@server
   ```
   then open `http://127.0.0.1:8787/`.

2. **Tailscale / WireGuard** — bind to the tailnet address:
   ```bash
   JARVIS_BIND=100.x.y.z:8787
   ```

3. **Reverse proxy with TLS** — Caddy in front of the loopback bind:
   ```
   jarvis.example.com {
       reverse_proxy 127.0.0.1:8787
   }
   ```
   Set `JARVIS_SECURE_COOKIE=1` and `JARVIS_ALLOWED_HOST=jarvis.example.com`.

## What works

Everything the desktop does, minus the pieces that require a desktop:

- chat, streaming, tool calls, approvals, input requests
- conversation search, rename, pin, delete, export
- provider setup, key entry, model listing, connection test
- MCP servers: add, edit, test, refresh, list tools, run a tool, permissions
- ACP agents: registry, enable, probe, per-chat switching, model/mode pickers
- workspace browsing and file previews
- file attachments, including drag and drop and pasted images
- background turns: closing the browser does not stop a reply

## Known gaps

| Gap | Why |
|---|---|
| No in-app update | there is no installer to swap; redeploy the binary |
| MCP OAuth needs its callback port reachable | the flow binds loopback on *this* host, so finish it here or forward the port |
| `open_data_dir` / `read_clipboard_image` | those are desktop concepts; paste images instead |
| Single user | there is one database and one token by design |
| Secrets are a 0600 file, not a keychain | headless hosts have no Secret Service |
| No WebSocket transport | SSE is enough for one-way events |

## Data and backup

```
$JARVIS_DATA_DIR/
  mcp-chat.db      conversations, messages, providers, servers, settings
  attachments/     uploaded files
  tool-output/     spilled tool results
  secrets.json     API keys and MCP tokens (0600)
  server-token     the access token
  uploads/         transient, cleaned per upload
```

Back up the whole directory. It is safe to copy while the server runs because
SQLite is in WAL mode, but stopping the service is simpler.
