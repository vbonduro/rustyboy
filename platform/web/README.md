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

Three auth modes are supported. Only one should be active at a time.

**Mode 1 — Google OAuth** (default, no extra flags needed)

| Variable | Description |
|---|---|
| `GOOGLE_CLIENT_ID` | OAuth 2.0 client ID from Google Cloud Console |
| `GOOGLE_CLIENT_SECRET` | OAuth 2.0 client secret |

The redirect URI is auto-detected from the request's `Host` header — no configuration needed. Register `https://yoursite.com/auth/google/callback` in Google Console (replacing `yoursite.com` with your actual domain).

**Mode 2 — Cloudflare Access** (recommended for Cloudflare Tunnel deployments)

Users authenticate at the Cloudflare edge; the server validates the injected JWT. No Google credentials needed on the server.

| Variable | Description |
|---|---|
| `CF_ACCESS_AUD` | Application Audience tag from your Cloudflare Access application settings |
| `CF_TEAM_DOMAIN` | Your Cloudflare team name, e.g. `mycompany` → `mycompany.cloudflareaccess.com` |

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

Never pass secrets directly on the command line — they appear in shell history and `docker inspect` output. Instead use a single secrets file mounted into the container.

### Setup

```sh
mkdir -p /path/to/appdata/rustyboy
cat > /path/to/appdata/rustyboy/secrets.env <<EOF
JWT_SECRET=<long random string>
GOOGLE_CLIENT_ID=<from Google Console>
GOOGLE_CLIENT_SECRET=<from Google Console>
EOF
chmod 600 /path/to/appdata/rustyboy/secrets.env
```

```sh
docker run \
  -v /path/to/appdata/rustyboy:/appdata:ro \
  -e SECRETS_FILE=/appdata/secrets.env \
  -p 8080:8080 \
  -v /path/to/roms:/roms \
  ghcr.io/vbonduro/rustyboy:1.x.x
```

The file uses standard `KEY=VALUE` format. Lines starting with `#` are ignored. `DEV_MODE` should never appear in a production secrets file.

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
