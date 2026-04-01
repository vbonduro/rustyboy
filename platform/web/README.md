# rustyboy — web platform

A browser-based Game Boy emulator that looks and feels like a real DMG. The emulator core compiles to WASM and runs entirely client-side; the Axum server handles authentication, ROM serving, and static files.

## How it works

```
Browser                          Server (Axum)
───────                          ─────────────
GET /                      →     serves index.html
GET /api/roms              →     lists .gb/.gbc files in ROMS_DIR
GET /api/me                →     returns current user info (requires session)
GET /api/auth-method       →     returns active auth mode: "google" | "cf" | "dev"
GET /roms/:name            →     streams ROM bytes (requires session)
GET /static/*              →     serves WASM + JS + CSS
GET /auth/google           →     begins Google OAuth flow
GET /auth/google/callback  →     completes Google OAuth, sets session cookie
GET /auth/cf-access        →     validates Cloudflare Access JWT, sets session cookie
POST /auth/logout          →     clears session cookie

JS loads rustyboy_web_client.wasm
  → new EmulatorHandle(romBytes)   (Rust/WASM)
  → requestAnimationFrame loop:
      handle.run_frame()           advances 70,224 T-cycles (~1 DMG frame)
      handle.framebuffer_rgba()    returns 160×144 RGBA pixels
      ctx.putImageData(...)        draws to <canvas>
  → touch/keyboard events:
      handle.set_button(idx, pressed)
```

Multiple users can run simultaneously — each browser tab has its own independent WASM instance. There are no server-side save states.

## Controls

| Button | Keyboard | Touch |
|---|---|---|
| D-pad | Arrow keys | On-screen D-pad |
| A | Z | A button |
| B | X | B button |
| Start | Enter | START button |
| Select | Shift | SELECT button |
| Menu (power) | Backspace | ⏻ button |

## Building and running locally

### Prerequisites

- Rust (stable) — https://rustup.rs
- wasm-pack — `curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh`

### Steps

```sh
# 1. Build the WASM client
wasm-pack build platform/web/client \
  --target web \
  --out-dir platform/web/client/static

# 2. Build and run the server (DEV_MODE skips auth for local testing)
ROMS_DIR=/path/to/your/roms \
STATIC_DIR=platform/web/client/static \
JWT_SECRET=local-dev-secret \
DEV_MODE=1 \
cargo run -p rustyboy-web-server

# 3. Open http://localhost:8080
```

ROMs must be `.gb` or `.gbc` files. The server lists whatever is in `ROMS_DIR`.

For local Docker testing with auth bypassed:

```sh
docker run -p 8080:8080 \
  -v /path/to/your/roms:/roms \
  -e JWT_SECRET=local-dev-secret \
  -e DEV_MODE=1 \
  ghcr.io/vbonduro/rustyboy:dev
```

## Docker

The Dockerfile performs a full multi-stage build — no local toolchain required.

```sh
# Build the image (takes a few minutes the first time)
docker build -f platform/web/Dockerfile -t rustyboy-web .

# Run — mount your ROMs directory
docker run -p 8080:8080 -v /path/to/your/roms:/roms rustyboy-web
```

Open `http://localhost:8080` in a browser.

### Environment variables

#### Core

| Variable | Default | Description |
|---|---|---|
| `ROMS_DIR` | `/roms` | Directory scanned for `.gb`/`.gbc` ROM files |
| `STATIC_DIR` | `/static` | Directory serving the built frontend assets |
| `PORT` | `8080` | Port the server listens on |
| `DB_PATH` | `/data/rustyboy.db` | SQLite database path (user accounts) |
| `JWT_SECRET` | _(required)_ | Secret used to sign session cookies — set to a long random string |
| `RUST_LOG` | _(unset)_ | Log level, e.g. `info` |

#### Authentication

All secrets go in `secrets.env` (see [Managing secrets](#managing-secrets) below). Three auth modes are supported; Google OAuth and Cloudflare Access can be active simultaneously.

**Mode 1 — Google OAuth**

1. Go to [Google Cloud Console](https://console.cloud.google.com) → APIs & Services → Credentials → **Create OAuth 2.0 Client ID** (type: Web application)
2. Under **Authorized JavaScript origins** add: `https://yoursite.com`
3. Under **Authorized redirect URIs** add: `https://yoursite.com/auth/google/callback`
4. Copy the Client ID and Client Secret into `secrets.env`

When you click "Sign in with Google", the server builds the OAuth redirect URI from the incoming request's `Host` header (e.g. `https://rustyboy.yourdomain.com/auth/google/callback`). This works automatically when accessed through a named domain like a Cloudflare tunnel.

**Accessing via local IP alongside a Cloudflare tunnel**

Google does not allow private IP addresses as OAuth redirect URIs, so initiating the OAuth flow from `http://192.168.x.x:8080` will fail with a `redirect_uri` error. Set `OAUTH_REDIRECT_URI` to your tunnel's callback URL to fix this:

```sh
OAUTH_REDIRECT_URI=https://rustyboy.yourdomain.com/auth/google/callback
```

When this is set and a request arrives from a bare IP, the server sends the configured URI to Google instead of building one from the IP. Google then redirects the browser back to your tunnel URL to complete the auth — which proxies to your server and sets the session cookie as normal. The end result is the same regardless of whether you initiated the flow from the tunnel or the local IP.

| Variable | Where to find it |
|---|---|
| `GOOGLE_CLIENT_ID` | Google Cloud Console → Credentials → your OAuth client → Client ID (ends in `.apps.googleusercontent.com`) |
| `GOOGLE_CLIENT_SECRET` | Same page → Client Secret (starts with `GOCSPX-`) |
| `OAUTH_REDIRECT_URI` | Your tunnel callback URL — only needed when you also access the server via local IP. Set to `https://yoursite.com/auth/google/callback` |

**Mode 2 — Cloudflare Access** (recommended for Cloudflare Tunnel deployments)

Users authenticate at the Cloudflare edge; the server validates the injected JWT. Can be used alongside Google OAuth.

1. In the Cloudflare dashboard create a Tunnel pointing to `http://<server-ip>:8080`
2. Create an Access Application protecting that tunnel
3. Copy the Audience tag and team name into `secrets.env`

| Variable | Where to find it |
|---|---|
| `CF_ACCESS_AUD` | Cloudflare dashboard → Access → Applications → your app → **Audience tag** |
| `CF_TEAM_DOMAIN` | Your Cloudflare team name — if your team URL is `mycompany.cloudflareaccess.com` use `mycompany` |

**Mode 3 — Dev mode** (local development only, bypasses all auth)

| Variable | Description |
|---|---|
| `DEV_MODE` | Set to any value to skip authentication and auto-login as `dev@localhost` |

## Docker images

Two images are published to the GitHub Container Registry:

| Image | Published on | Use for |
|---|---|---|
| `ghcr.io/vbonduro/rustyboy:1.x.x` | GitHub release | Production — pinned, stable |
| `ghcr.io/vbonduro/rustyboy:dev` | Every push to `main` | Development / testing — always latest |

Use a pinned semver tag for production so a bad push never disrupts your server.

## Managing secrets

Never pass secrets directly on the command line — they appear in shell history and `docker inspect` output. Instead put all secrets in a single file mounted into the container via `SECRETS_FILE`.

### Setup

```sh
mkdir -p /path/to/appdata/rustyboy
chmod 600 /path/to/appdata/rustyboy/secrets.env
```

Edit `/path/to/appdata/rustyboy/secrets.env`:

```sh
# Required — generate with: openssl rand -hex 32
JWT_SECRET=<long random string>

# Google OAuth
# Find at: console.cloud.google.com → APIs & Services → Credentials → your OAuth client
GOOGLE_CLIENT_ID=<ends in .apps.googleusercontent.com>
GOOGLE_CLIENT_SECRET=<starts with GOCSPX->
# Only needed if you access the server via local IP as well as a tunnel/domain.
# Set to your tunnel's callback URL so Google has a valid redirect target.
OAUTH_REDIRECT_URI=https://yoursite.com/auth/google/callback

# Cloudflare Access
# CF_ACCESS_AUD: Cloudflare dashboard → Access → Applications → your app → Audience tag
# CF_TEAM_DOMAIN: your team name — if your URL is mycompany.cloudflareaccess.com use "mycompany"
CF_ACCESS_AUD=<audience tag>
CF_TEAM_DOMAIN=<team name>
```

```sh
docker run \
  -v /path/to/appdata/rustyboy:/appdata:ro \
  -e SECRETS_FILE=/appdata/secrets.env \
  -p 8080:8080 \
  -v /path/to/roms:/roms \
  ghcr.io/vbonduro/rustyboy:1.x.x
```

The file uses standard `KEY=VALUE` format. Lines starting with `#` are ignored. Only include the variables for the auth modes you are using — unused variables are safely ignored. `DEV_MODE` should never appear in a production secrets file.

## Deploying on Unraid

### Prerequisites

- Unraid 6.10 or later with the **Community Applications** plugin
- Your ROM files accessible on the Unraid array

### Install via template

1. Copy the template to Unraid's template directory:

   ```sh
   # Run in the Unraid terminal
   wget -O /boot/config/plugins/dockerMan/templates-user/rustyboy.xml \
     https://raw.githubusercontent.com/vbonduro/rustyboy/main/deploy/unraid/rustyboy.xml
   ```

2. In the Unraid UI go to **Docker → Add Container** and select **rustyboy** from the template list.

3. Set the **ROMs** path to the folder on your array containing your `.gb` and `.gbc` files, e.g.:

   ```
   /mnt/user/Games/GameBoy
   ```

4. Create your secrets file and set `SECRETS_FILE=/appdata/secrets.env` (see Managing secrets above).

5. Click **Apply**. The container pulls `ghcr.io/vbonduro/rustyboy:latest` and starts automatically.

6. Open `http://<unraid-ip>:8080` in a browser — your ROMs appear in the menu.

### Updating

In the Unraid Docker tab, click the rustyboy container icon and choose **Update**. Or enable **Auto Update** via Unraid's settings.

## Directory layout

```
platform/web/
├── Dockerfile
├── client/
│   ├── Cargo.toml          # cdylib crate (wasm-bindgen)
│   ├── src/lib.rs          # EmulatorHandle: new / run_frame / framebuffer_rgba / set_button
│   └── static/
│       ├── index.html      # DMG Game Boy shell
│       ├── style.css       # Pixel-accurate DMG styling
│       ├── app.js          # ROM menu, emulation loop, input handling, auth flow
│       └── menu.js         # MenuRenderer — GB-palette canvas menus
├── e2e/
│   ├── playwright.config.js
│   ├── server.cjs          # Mock server for E2E tests
│   └── tests/
│       └── traversal.spec.js  # Menu traversal + mobile interaction tests
└── server/
    ├── Cargo.toml
    └── src/
        ├── main.rs         # Entrypoint, env config
        ├── lib.rs          # Axum router, middleware, route handlers
        ├── auth.rs         # Google OAuth, Cloudflare Access, session JWT
        └── db.rs           # SQLite user store (sqlx)
```
