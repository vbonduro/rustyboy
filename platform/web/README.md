# rustyboy — web platform

A browser-based Game Boy emulator that looks and feels like a real DMG. The emulator core compiles to WASM and runs entirely client-side; the server is stateless and just serves static files and ROM downloads.

## How it works

```
Browser                          Server (Axum)
───────                          ─────────────
GET /                      →     serves index.html
GET /api/roms              →     lists .gb/.gbc files in ROMS_DIR
GET /roms/:name            →     streams ROM bytes
GET /static/*              →     serves WASM + JS + CSS

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

# 2. Build and run the server
ROMS_DIR=/path/to/your/roms \
STATIC_DIR=platform/web/client/static \
cargo run -p rustyboy-web-server

# 3. Open http://localhost:8080
```

ROMs must be `.gb` or `.gbc` files. The server lists whatever is in `ROMS_DIR`.

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

| Variable | Default | Description |
|---|---|---|
| `ROMS_DIR` | `/roms` | Directory scanned for `.gb`/`.gbc` ROM files |
| `STATIC_DIR` | `/static` | Directory serving the built frontend assets |
| `RUST_LOG` | _(unset)_ | Log level, e.g. `info` |

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

4. Click **Apply**. The container pulls `ghcr.io/vbonduro/rustyboy:latest` and starts automatically.

5. Open `http://<unraid-ip>:8080` in a browser — your ROMs appear in the menu.

### Updating

In the Unraid Docker tab, click the rustyboy container icon and choose **Update**. Or enable **Auto Update** via Unraid's settings.

### Network access

The server binds on port 8080. If you want to play from other devices on your LAN, make sure nothing is blocking that port on the Unraid host. No authentication is built in — keep it on your local network or behind a VPN.

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
│       └── app.js          # ROM menu, emulation loop, input handling
└── server/
    ├── Cargo.toml
    └── src/main.rs         # Axum: GET /, /api/roms, /roms/:name, /static/*
```
