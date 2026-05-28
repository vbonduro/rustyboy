//! cargo xtask — unified build, run, deploy, setup, and crash-decode for rustyboy.
//!
//! Invoke from the workspace root:
//!
//!   cargo xtask build  {web|pico}
//!   cargo xtask deploy {web|pico}
//!   cargo xtask run    pico
//!   cargo xtask crash  pico         ← pull + decode crash log via USB (no probe needed)
//!   cargo xtask setup  {web|pico}

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::io::{self, BufRead, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// ── CLI schema ────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "xtask", about = "rustyboy build & deploy tasks")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Build a platform target
    Build {
        #[command(subcommand)]
        target: BuildTarget,
    },
    /// Deploy a platform target
    Deploy {
        #[command(subcommand)]
        target: DeployTarget,
    },
    /// Flash firmware and stream RTT logs via SWD debug probe
    Run {
        #[command(subcommand)]
        target: RunTarget,
    },
    /// Pull the crash log from the device and decode it (no SWD probe needed)
    Crash {
        #[command(subcommand)]
        target: CrashTarget,
    },
    /// One-time setup: install required tools and configure the system
    Setup {
        #[command(subcommand)]
        target: SetupTarget,
    },
}

#[derive(Subcommand)]
enum BuildTarget {
    /// Build the rustyboy-web Docker image
    Web,
    /// Cross-compile the pico2w firmware (release, ARM Cortex-M33)
    Pico,
}

#[derive(Subcommand)]
enum DeployTarget {
    /// Build image, (re)start container, and print the URL
    Web,
    /// Build firmware and flash over USB BOOTSEL — no SWD probe needed
    Pico,
}

#[derive(Subcommand)]
enum RunTarget {
    /// Flash pico2w via SWD probe and stream defmt RTT logs (Ctrl-C to stop)
    Pico,
}

#[derive(Subcommand)]
enum CrashTarget {
    /// Pull crash log via USB (BOOTSEL) + decode with crash_decoder.py
    Pico,
}

#[derive(Subcommand)]
enum SetupTarget {
    /// Install Docker Engine and add current user to the docker group
    Web,
    /// Build + install picotool from source; write udev rules for Raspberry Pi USB devices
    Pico,
}

// ── entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = workspace_root();

    match cli.command {
        Cmd::Build { target } => match target {
            BuildTarget::Web => build_web(&root),
            BuildTarget::Pico => build_pico(&root),
        },
        Cmd::Deploy { target } => match target {
            DeployTarget::Web => deploy_web(&root),
            DeployTarget::Pico => deploy_pico(&root),
        },
        Cmd::Run { target } => match target {
            RunTarget::Pico => run_pico(&root),
        },
        Cmd::Crash { target } => match target {
            CrashTarget::Pico => crash_pico(&root),
        },
        Cmd::Setup { target } => match target {
            SetupTarget::Web => setup_web(),
            SetupTarget::Pico => setup_pico(),
        },
    }
}

// ── build ─────────────────────────────────────────────────────────────────────

fn build_web(root: &Path) -> Result<()> {
    require_tool("docker", "Run `cargo xtask setup web` to install Docker.")?;
    println!("🔨 Building rustyboy-web Docker image…");
    cmd(
        "docker",
        &["build", "-f", "platform/web/Dockerfile", "-t", "rustyboy-web", "."],
        root,
    )
}

fn build_pico(root: &Path) -> Result<()> {
    println!("🔨 Cross-compiling pico2w firmware (release)…");
    // Run from platform/pico2w/ so cargo picks up its .cargo/config.toml,
    // which sets target = thumbv8m.main-none-eabihf and the correct linker flags.
    // The workspace target dir (<root>/target/) is still used for the output.
    cmd(
        "cargo",
        &["build", "--release"],
        &root.join("platform/pico2w"),
    )
}

// ── deploy ────────────────────────────────────────────────────────────────────

fn deploy_web(root: &Path) -> Result<()> {
    build_web(root)?;

    println!("🧹 Removing existing rustyboy-web container (if any)…");
    cmd_best_effort("docker", &["rm", "-f", "rustyboy-web"], root);

    let appdata = appdata_dir();
    std::fs::create_dir_all(&appdata)
        .with_context(|| format!("failed to create appdata dir: {}", appdata.display()))?;

    let roms_mount = format!("{}:/roms:ro", root.join("roms").display());
    let data_mount = format!("{}:/appdata", appdata.display());

    println!("🚀 Starting rustyboy-web container…");
    cmd(
        "docker",
        &[
            "run",
            "-d",
            "--name",
            "rustyboy-web",
            "-p",
            "8080:8080",
            "-v",
            &roms_mount,
            "-v",
            &data_mount,
            "rustyboy-web",
        ],
        root,
    )?;

    let ip = local_ip();
    println!("\n🎮  rustyboy is running → \x1b[1;32mhttp://{ip}:8080\x1b[0m");
    Ok(())
}

fn deploy_pico(root: &Path) -> Result<()> {
    require_tool(
        "picotool",
        "Run `cargo xtask setup pico` to build and install picotool.",
    )?;

    build_pico(root)?;
    wait_for_bootsel()?;

    // ELF lands in the workspace target dir (shared by all workspace members).
    let elf = root.join("target/thumbv8m.main-none-eabihf/release/rustyboy-pico2w");

    println!("⚡ Flashing via picotool…");
    cmd("picotool", &["load", "-f", "-t", "elf", elf.to_str().unwrap()], root)?;
    cmd("picotool", &["reboot"], root)?;
    println!("✅ Done — Pico 2W is rebooting into the new firmware.");
    Ok(())
}

// ── run ───────────────────────────────────────────────────────────────────────

fn run_pico(root: &Path) -> Result<()> {
    require_tool(
        "probe-rs",
        "Install with: cargo install probe-rs-tools --locked",
    )?;

    // A stale probe-rs process holds the USB device and causes "Probe not found".
    println!("🔍 Releasing any stale probe-rs process…");
    cmd_best_effort("pkill", &["-f", "probe-rs"], root);
    std::thread::sleep(Duration::from_secs(1));

    println!("🔨 Building + flashing via SWD probe — defmt RTT logs streaming below…");
    println!("   (Ctrl-C to stop)\n");
    // The runner in platform/pico2w/.cargo/config.toml is `probe-rs run --chip RP235x`.
    cmd(
        "cargo",
        &["run", "--release"],
        &root.join("platform/pico2w"),
    )
}

// ── crash ─────────────────────────────────────────────────────────────────────

fn crash_pico(root: &Path) -> Result<()> {
    require_tool(
        "picotool",
        "Run `cargo xtask setup pico` to build and install picotool.",
    )?;

    // The crash sector is the last 4 KiB of flash (XIP address 0x103FF000).
    // picotool can save it while the device is in BOOTSEL mode — no probe needed.
    println!("📋 This will read the crash log from flash via USB.");
    println!("   Put the Pico 2W into BOOTSEL mode first:");
    println!("   hold BOOTSEL + press RESET (or unplug/replug while holding BOOTSEL).");
    wait_for_bootsel()?;

    let crash_bin = root.join("crash.bin");

    println!("💾 Saving crash sector (0x103FF000–0x10400000, 4 KiB)…");
    // picotool save -r <from> <to> <filename>  (no -o flag; range end is exclusive)
    cmd(
        "picotool",
        &[
            "save",
            "-r",
            "0x103FF000",
            "0x10400000",
            crash_bin.to_str().unwrap(),
        ],
        root,
    )?;

    println!("🔍 Decoding…\n");
    let elf = root.join("target/thumbv8m.main-none-eabihf/release/rustyboy-pico2w");
    let mut decode_args = vec![
        "run".to_string(),
        "--script".to_string(),
        "tools/crash_decoder.py".to_string(),
        "--raw".to_string(),
        crash_bin.to_str().unwrap().to_string(),
    ];
    if elf.exists() {
        decode_args.push("--elf".to_string());
        decode_args.push(elf.to_str().unwrap().to_string());
    } else {
        println!("⚠️  ELF not found — symbolisation disabled. Build first for source locations.");
    }

    let args_refs: Vec<&str> = decode_args.iter().map(String::as_str).collect();
    cmd("uv", &args_refs, root)?;

    // Clean up the raw binary — the decoded output is what matters.
    let _ = std::fs::remove_file(&crash_bin);
    Ok(())
}

// ── setup: web ────────────────────────────────────────────────────────────────

fn setup_web() -> Result<()> {
    if tool_available("docker") {
        println!("✅ Docker is already installed.");
        return Ok(());
    }

    println!("📦 Installing Docker Engine via get.docker.com (requires sudo)…");
    let status = Command::new("sh")
        .args(["-c", "curl -fsSL https://get.docker.com | sudo sh"])
        .status()
        .context("failed to launch Docker install script")?;
    if !status.success() {
        bail!("Docker install script failed — check the output above for details.");
    }

    let user = current_user();
    cmd_best_effort("sudo", &["usermod", "-aG", "docker", &user], Path::new("/"));

    println!("\n✅ Docker installed.");
    println!(
        "⚠️  Log out and back in (or run `newgrp docker`) for group membership to take effect."
    );
    Ok(())
}

// ── setup: pico ───────────────────────────────────────────────────────────────

fn setup_pico() -> Result<()> {
    if tool_available("picotool") {
        println!("✅ picotool is already installed.");
    } else {
        install_picotool()?;
    }

    write_udev_rules()?;
    reload_udev();

    let user = current_user();
    cmd_best_effort("sudo", &["usermod", "-aG", "plugdev", &user], Path::new("/"));

    println!("\n✅ pico setup complete.");
    println!(
        "⚠️  Log out and back in (or run `newgrp plugdev`) for group membership to take effect."
    );
    println!("   Hold BOOTSEL + plug in USB, then run: cargo xtask deploy pico");
    Ok(())
}

fn install_picotool() -> Result<()> {
    println!("📦 Installing picotool build dependencies…");
    cmd(
        "sudo",
        &[
            "apt-get",
            "install",
            "-y",
            "cmake",
            "libusb-1.0-0-dev",
            "pkg-config",
            "build-essential",
            "git",
        ],
        Path::new("/tmp"),
    )?;

    let build_dir = PathBuf::from("/tmp/picotool-build");
    if build_dir.exists() {
        std::fs::remove_dir_all(&build_dir)?;
    }

    // picotool requires the Pico SDK — clone it alongside picotool.
    let sdk_dir = PathBuf::from("/tmp/pico-sdk");
    if sdk_dir.exists() {
        std::fs::remove_dir_all(&sdk_dir)?;
    }
    println!("📥 Cloning Pico SDK…");
    cmd(
        "git",
        &[
            "clone",
            "--depth",
            "1",
            "https://github.com/raspberrypi/pico-sdk.git",
            sdk_dir.to_str().unwrap(),
        ],
        Path::new("/tmp"),
    )?;

    println!("📥 Cloning picotool…");
    cmd(
        "git",
        &[
            "clone",
            "--depth",
            "1",
            "https://github.com/raspberrypi/picotool.git",
            build_dir.to_str().unwrap(),
        ],
        Path::new("/tmp"),
    )?;

    let cmake_build = build_dir.join("build");
    std::fs::create_dir_all(&cmake_build)?;

    let sdk_path_arg = format!("-DPICO_SDK_PATH={}", sdk_dir.display());
    println!("🔨 Building picotool…");
    cmd(
        "cmake",
        &["..", &sdk_path_arg],
        &cmake_build,
    )?;

    let jobs = nproc().to_string();
    cmd("make", &["-j", &jobs], &cmake_build)?;

    println!("📦 Installing picotool (requires sudo)…");
    cmd("sudo", &["make", "install"], &cmake_build)?;

    println!("✅ picotool installed.");
    Ok(())
}

fn write_udev_rules() -> Result<()> {
    const RULES: &str = "# Raspberry Pi Pico / Pico 2 USB rules — written by `cargo xtask setup pico`\n\
        # RP2040 BOOTSEL mode\n\
        SUBSYSTEM==\"usb\", ATTRS{idVendor}==\"2e8a\", ATTRS{idProduct}==\"0003\", MODE=\"0666\", GROUP=\"plugdev\"\n\
        # RP2350 BOOTSEL mode\n\
        SUBSYSTEM==\"usb\", ATTRS{idVendor}==\"2e8a\", ATTRS{idProduct}==\"000f\", MODE=\"0666\", GROUP=\"plugdev\"\n\
        # Raspberry Pi Debug Probe (CMSIS-DAP)\n\
        SUBSYSTEM==\"usb\", ATTRS{idVendor}==\"2e8a\", ATTRS{idProduct}==\"000c\", MODE=\"0666\", GROUP=\"plugdev\"\n";

    println!("📝 Writing /etc/udev/rules.d/60-pico.rules…");

    // Pipe to `sudo tee` so we don't need to handle privilege escalation ourselves.
    let mut child = Command::new("sudo")
        .args(["tee", "/etc/udev/rules.d/60-pico.rules"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .context("failed to spawn `sudo tee`")?;

    child
        .stdin
        .take()
        .unwrap()
        .write_all(RULES.as_bytes())
        .context("failed to write udev rules")?;

    let status = child.wait()?;
    if !status.success() {
        bail!("failed to write udev rules — check sudo permissions");
    }
    Ok(())
}

fn reload_udev() {
    cmd_best_effort(
        "sudo",
        &["udevadm", "control", "--reload-rules"],
        Path::new("/"),
    );
    cmd_best_effort("sudo", &["udevadm", "trigger"], Path::new("/"));
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Run `prog args…` in `cwd`, inheriting stdio. Errors if the process exits non-zero.
fn cmd(prog: &str, args: &[&str], cwd: &Path) -> Result<()> {
    let status = Command::new(prog)
        .args(args)
        .current_dir(cwd)
        .status()
        .with_context(|| format!("failed to spawn `{prog}`"))?;
    if !status.success() {
        bail!("`{prog} {}` exited with {status}", args.join(" "));
    }
    Ok(())
}

/// Like `cmd` but silently ignores failures (for best-effort cleanup steps).
fn cmd_best_effort(prog: &str, args: &[&str], cwd: &Path) {
    let _ = Command::new(prog).args(args).current_dir(cwd).status();
}

/// Returns true if `name` is on PATH and can be spawned.
///
/// Deliberately ignores exit code — some tools (e.g. picotool) return
/// non-zero when given no arguments or `--version`. The only failure we
/// care about is ENOENT ("binary not found"), which surfaces as an Err.
fn tool_available(name: &str) -> bool {
    Command::new(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok()
}

/// Asserts a tool is available; bails with a human-readable hint if it isn't.
fn require_tool(name: &str, hint: &str) -> Result<()> {
    if tool_available(name) {
        Ok(())
    } else {
        bail!("`{name}` not found on PATH.\n  {hint}");
    }
}

/// Prompt the user to put the Pico 2W into BOOTSEL mode and wait until detected.
fn wait_for_bootsel() -> Result<()> {
    if bootsel_detected() {
        println!("✅ Pico 2W detected in BOOTSEL mode.");
        return Ok(());
    }

    println!("\n⚡ Pico 2W not found in BOOTSEL mode.");
    println!("   1. Unplug the Pico 2W from USB (if connected).");
    println!("   2. Hold the BOOTSEL button on the Pico 2W.");
    println!("   3. Plug the Pico 2W into USB while still holding BOOTSEL.");
    println!("   4. Release BOOTSEL.");
    print!("\n   Press Enter when ready… ");
    io::stdout().flush()?;
    let stdin = io::stdin();
    let _ = stdin.lock().lines().next();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if bootsel_detected() {
            println!("✅ Pico 2W detected in BOOTSEL mode.");
            return Ok(());
        }
        if Instant::now() > deadline {
            bail!(
                "Timed out waiting for Pico 2W in BOOTSEL mode.\n\
                 Make sure you hold BOOTSEL while plugging in USB."
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Returns true if a Pico in BOOTSEL mode is visible on USB.
///
/// USB IDs:
///   2e8a:000f — RP2350 (Pico 2 / Pico 2W) BOOTSEL
///   2e8a:0003 — RP2040 (Pico / Pico W)    BOOTSEL
fn bootsel_detected() -> bool {
    Command::new("lsusb")
        .output()
        .map(|o| {
            let s = String::from_utf8_lossy(&o.stdout);
            s.contains("2e8a:000f") || s.contains("2e8a:0003")
        })
        .unwrap_or(false)
}

/// Returns the first non-loopback IP address reported by `hostname -I`.
fn local_ip() -> String {
    Command::new("hostname")
        .arg("-I")
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8(o.stdout)
                .ok()?
                .split_whitespace()
                .next()
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "localhost".to_owned())
}

/// Persistent application data directory: $XDG_DATA_HOME/rustyboy or ~/.local/share/rustyboy.
fn appdata_dir() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".local/share"));
    base.join("rustyboy")
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

fn current_user() -> String {
    std::env::var("USER").unwrap_or_else(|_| "root".to_owned())
}

fn nproc() -> usize {
    Command::new("nproc")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(4)
}

/// Returns the workspace root — the parent of the `xtask/` crate directory.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask Cargo.toml must have a parent directory")
        .to_owned()
}
