# Packaging & Install Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `herdr plugin install vandemaelefelix/herdr-pets` succeed with **no Rust toolchain** on supported platforms (prebuilt binary), falling back to a source build everywhere else, and give the README clear install instructions.

**Architecture:** A tag-triggered GitHub Actions workflow cross-compiles four target binaries and attaches them to a GitHub Release. `scripts/build.sh` (herdr's `[[build]]` step) detects the platform, downloads the matching binary, and only runs `cargo build` if the download is unavailable. README + manifest are refreshed to match.

**Tech Stack:** POSIX `sh`, GitHub Actions, `cargo`, Rust integration tests (`std::process::Command`).

Spec: [`docs/superpowers/specs/2026-07-26-packaging-and-install-design.md`](../specs/2026-07-26-packaging-and-install-design.md).

## Global Constraints

- **Rust toolchain pin:** `1.96` (matches `.github/workflows/ci.yml` and `Cargo.toml` `rust-version`). Any new workflow installs exactly `1.96`.
- **Release repo:** `vandemaelefelix/herdr-pets`. Tags are `v<version>` (e.g. `v0.1.0`).
- **Plugin version:** `0.1.0` — this exact string appears in `Cargo.toml`, `herdr-plugin.toml`, `tests/manifest.rs`, and the URL `build.sh` builds. All must agree at release time.
- **Four published targets:** `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`. Release asset names are `herdr-pets-<target>` (raw executable, no archive).
- **`build.sh` is POSIX `sh`** — no bash-isms; only `uname`, `sed`, `curl`, `mkdir`, `chmod`, `mv`, `rm` (all present on stock macOS/Linux).
- **Error rule** (`.claude/skills/rust-error-handling`): degrade at the boundary. `build.sh` treats any failure of the preferred path as a reason to try the next; it exits non-zero only if **both** fetch and source build fail.
- **Gate stays green:** `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` + `cargo test` after every task. `ci.yml` is **not** modified.
- **No new features / knobs / dependencies.** Packaging only.
- Branch is `chore/packaging-and-install` (already created off `main`).

---

### Task 1: Fetch-or-build `scripts/build.sh` + dry-run tests

Rewrites the build step to prefer a prebuilt download and fall back to source. A `--print-url` dry-run makes the risky `uname → target → URL` logic testable without any network access.

**Files:**
- Modify: `scripts/build.sh` (full rewrite)
- Test: `tests/build_script.rs` (create)

**Interfaces:**
- Produces: `scripts/build.sh --print-url` prints the resolved download URL to stdout (empty line if the platform is unsupported) and exits `0`. Platform detection honors `HERDR_PETS_FAKE_UNAME_S` / `HERDR_PETS_FAKE_UNAME_M` env overrides (used by the test); absent those, it uses real `uname -s` / `uname -m`. Version is read from `herdr-plugin.toml`'s `version = "..."`.
- Consumes: nothing from earlier tasks.

- [ ] **Step 1: Write the failing test**

Create `tests/build_script.rs`:

```rust
//! Dry-run checks for scripts/build.sh: the uname -> target-triple -> URL map.
//! `--print-url` resolves the download URL without touching the network, so the
//! risky platform-detection logic is verified deterministically here.

use std::process::Command;

/// Run `build.sh --print-url` with a faked platform and return the trimmed URL.
fn print_url(os: &str, arch: &str) -> String {
    let dir = env!("CARGO_MANIFEST_DIR");
    let out = Command::new("sh")
        .arg(format!("{dir}/scripts/build.sh"))
        .arg("--print-url")
        .env("HERDR_PETS_FAKE_UNAME_S", os)
        .env("HERDR_PETS_FAKE_UNAME_M", arch)
        .current_dir(dir)
        .output()
        .expect("build.sh should run under sh");
    assert!(out.status.success(), "build.sh --print-url should exit 0");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn macos_arm64_maps_to_the_apple_silicon_asset_at_the_manifest_version() {
    let url = print_url("Darwin", "arm64");
    assert!(
        url.ends_with("/herdr-pets-aarch64-apple-darwin"),
        "got: {url}"
    );
    // Ties the URL's version to the manifest so version drift fails the gate.
    assert!(url.contains("/releases/download/v0.1.0/"), "got: {url}");
}

#[test]
fn macos_intel_maps_to_the_x86_64_apple_asset() {
    let url = print_url("Darwin", "x86_64");
    assert!(url.ends_with("/herdr-pets-x86_64-apple-darwin"), "got: {url}");
}

#[test]
fn linux_x86_64_maps_to_the_gnu_asset() {
    let url = print_url("Linux", "x86_64");
    assert!(
        url.ends_with("/herdr-pets-x86_64-unknown-linux-gnu"),
        "got: {url}"
    );
}

#[test]
fn linux_arm64_maps_to_the_aarch64_gnu_asset() {
    let url = print_url("Linux", "aarch64");
    assert!(
        url.ends_with("/herdr-pets-aarch64-unknown-linux-gnu"),
        "got: {url}"
    );
}

#[test]
fn unsupported_platform_yields_no_url_so_install_falls_back_to_source() {
    let url = print_url("FreeBSD", "riscv64");
    assert!(url.is_empty(), "expected empty URL, got: {url}");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test build_script`
Expected: FAIL — the current `build.sh` doesn't accept `--print-url`, so `print_url` returns a non-URL string (the script execs `cargo build`) and the assertions fail (or the command hangs on a real build; if so, that itself proves the flag is unimplemented).

- [ ] **Step 3: Rewrite `scripts/build.sh`**

Replace the entire contents of `scripts/build.sh` with:

```sh
#!/bin/sh
# herdr [[build]] step for herdr-pets.
#
# Preferred path: download the prebuilt release binary for this platform, so
# installing needs no Rust toolchain. Fallback: build from source with cargo
# (used on platforms we don't publish, or when the download is unavailable).
# Either way the binary lands at target/release/herdr-pets, where the manifest's
# pane/action commands point.
#
# Dry-run: `scripts/build.sh --print-url` prints the resolved download URL (an
# empty line when the platform is unsupported) and exits without downloading.
# tests/build_script.rs uses it; set HERDR_PETS_FAKE_UNAME_S / _M to override
# platform detection under test.
set -e

REPO="vandemaelefelix/herdr-pets"
BIN_DIR="target/release"
DEST="$BIN_DIR/herdr-pets"

# --- version from the manifest (single source of truth) ---------------------
manifest_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
VERSION=$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\(.*\)".*/\1/p' \
  "$manifest_dir/herdr-plugin.toml" | head -n1)

# --- detect target triple ---------------------------------------------------
os=${HERDR_PETS_FAKE_UNAME_S:-$(uname -s)}
arch=${HERDR_PETS_FAKE_UNAME_M:-$(uname -m)}
target=""
case "$os" in
Darwin)
  case "$arch" in
  arm64 | aarch64) target="aarch64-apple-darwin" ;;
  x86_64) target="x86_64-apple-darwin" ;;
  esac
  ;;
Linux)
  case "$arch" in
  x86_64) target="x86_64-unknown-linux-gnu" ;;
  aarch64 | arm64) target="aarch64-unknown-linux-gnu" ;;
  esac
  ;;
esac

url=""
if [ -n "$target" ] && [ -n "$VERSION" ]; then
  url="https://github.com/$REPO/releases/download/v$VERSION/herdr-pets-$target"
fi

# --- dry-run for tests -------------------------------------------------------
if [ "$1" = "--print-url" ]; then
  printf '%s\n' "$url"
  exit 0
fi

# --- source build fallback ---------------------------------------------------
build_from_source() {
  echo "herdr-pets: building from source with cargo" >&2
  # Source ~/.cargo/env so cargo is found on GUI / login-less launches.
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
  exec cargo build --release
}

# --- fetch, verify, else fall back ------------------------------------------
[ -n "$url" ] || build_from_source

echo "herdr-pets: fetching prebuilt $target" >&2
mkdir -p "$BIN_DIR"
tmp="$DEST.download"
if curl -fsSL "$url" -o "$tmp" 2>/dev/null && [ -s "$tmp" ]; then
  chmod +x "$tmp"
  mv "$tmp" "$DEST"
  # Trust the binary only if it actually runs on this machine.
  if "$DEST" --version >/dev/null 2>&1; then
    echo "herdr-pets: installed prebuilt binary" >&2
    exit 0
  fi
fi
rm -f "$tmp"
echo "herdr-pets: prebuilt unavailable, falling back to source build" >&2
build_from_source
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test build_script`
Expected: PASS — all five tests green.

- [ ] **Step 5: Confirm the script is still POSIX-clean and the real path is intact**

Run: `sh -n scripts/build.sh && sh scripts/build.sh --print-url`
Expected: no syntax error; prints this machine's real download URL (e.g. `.../v0.1.0/herdr-pets-aarch64-apple-darwin` on Apple Silicon).

- [ ] **Step 6: Commit**

```bash
git add scripts/build.sh tests/build_script.rs
git commit -m "feat(packaging): fetch prebuilt binary in build.sh, source-build fallback"
```

---

### Task 2: Tidy `herdr-plugin.toml`

Removes stale phase-era comments and makes the action/pane titles present-tense. No parsed field changes, so `tests/manifest.rs` stays green.

**Files:**
- Modify: `herdr-plugin.toml`
- Test: `tests/manifest.rs` (unchanged — run to confirm still green)

**Interfaces:**
- Consumes/Produces: nothing new. `id`, `version`, `platforms`, and all `command` arrays keep their current values (asserted by `tests/manifest.rs`).

- [ ] **Step 1: Rewrite the manifest**

Replace the entire contents of `herdr-plugin.toml` with:

```toml
# herdr-plugin.toml — manifest for herdr-pets.
id = "herdr-pets"
name = "herdr-pets"
version = "0.1.0"
description = "A herd of pixel-art pets for your herdr agents."
min_herdr_version = "0.7.0"
platforms = ["linux", "macos"]

# On install, herdr runs this step. build.sh downloads the prebuilt binary for
# the platform (no Rust needed) and falls back to `cargo build --release`.
[[build]]
platforms = ["linux", "macos"]
command = ["/bin/sh", "scripts/build.sh"]

# A pets strip opened manually via `herdr plugin pane open`.
[[panes]]
id = "pets"
title = "Pets"
placement = "split"
command = ["./target/release/herdr-pets", "render"]

# Place a full-width pets strip in the current tab now (destructive rebuild).
[[actions]]
id = "place-pets"
title = "Place pets strip"
command = ["./target/release/herdr-pets", "place"]

# Start the always-on watchdog that keeps a strip in every eligible tab.
[[actions]]
id = "start-pets-controller"
title = "Start pets controller"
command = ["./target/release/herdr-pets", "control"]
```

- [ ] **Step 2: Run the manifest tests to confirm nothing parsed-facing changed**

Run: `cargo test --test manifest`
Expected: PASS — all four `manifest_*` tests green.

- [ ] **Step 3: Commit**

```bash
git add herdr-plugin.toml
git commit -m "chore(packaging): tidy manifest comments and action titles"
```

---

### Task 3: Release workflow `.github/workflows/release.yml`

Adds a tag-triggered workflow that cross-compiles all four targets and publishes them to a GitHub Release. `ci.yml` is untouched.

**Files:**
- Create: `.github/workflows/release.yml`

**Interfaces:**
- Produces: on a pushed tag `v*`, a GitHub Release for that tag with assets named `herdr-pets-<target>` — the exact names `build.sh` downloads (Task 1).

- [ ] **Step 1: Create the workflow**

Create `.github/workflows/release.yml`:

```yaml
name: Release
on:
  push:
    tags: ["v*"]

permissions:
  contents: write

jobs:
  build:
    name: Build ${{ matrix.target }}
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - target: aarch64-apple-darwin
            os: macos-latest
          - target: x86_64-apple-darwin
            os: macos-latest
          - target: x86_64-unknown-linux-gnu
            os: ubuntu-latest
          - target: aarch64-unknown-linux-gnu
            os: ubuntu-latest
            apt: gcc-aarch64-linux-gnu
            linker: aarch64-linux-gnu-gcc
    steps:
      - uses: actions/checkout@v4
      - name: Install Rust 1.96 for ${{ matrix.target }}
        run: |
          rustup toolchain install 1.96 --profile minimal
          rustup default 1.96
          rustup target add ${{ matrix.target }}
      - name: Install cross linker
        if: matrix.apt != ''
        run: sudo apt-get update && sudo apt-get install -y ${{ matrix.apt }}
      - name: Configure cross linker
        if: matrix.linker != ''
        run: echo "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=${{ matrix.linker }}" >> "$GITHUB_ENV"
      - name: Build
        run: cargo build --release --target ${{ matrix.target }}
      - name: Stage binary
        run: |
          mkdir -p dist
          cp "target/${{ matrix.target }}/release/herdr-pets" "dist/herdr-pets-${{ matrix.target }}"
      - uses: actions/upload-artifact@v4
        with:
          name: herdr-pets-${{ matrix.target }}
          path: dist/herdr-pets-${{ matrix.target }}
          if-no-files-found: error

  publish:
    name: Publish release
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/download-artifact@v4
        with:
          path: dist
          merge-multiple: true
      - name: Create GitHub Release
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          gh release create "$GITHUB_REF_NAME" \
            --repo "$GITHUB_REPOSITORY" \
            --title "$GITHUB_REF_NAME" \
            --generate-notes \
            dist/herdr-pets-*
```

- [ ] **Step 2: Validate the workflow YAML parses**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml')); print('ok')"`
Expected: prints `ok` (no YAML error). If `actionlint` is installed, also run `actionlint .github/workflows/release.yml` and expect no findings.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci(packaging): add tag-triggered release workflow for four targets"
```

---

### Task 4: README refresh

Removes the stale "in design" banner, rewrites Install to lead with the one-liner and "no Rust needed", adds a Quickstart, and fixes the License section to MIT.

**Files:**
- Modify: `README.md`

**Interfaces:**
- Consumes: install command shape from the spec; the `start-pets-controller` action name (from the manifest, Task 2).

- [ ] **Step 1: Remove the "in design" status banner**

Delete this blockquote block near the top of `README.md` (lines ~9–13, immediately after the opening description paragraph):

```markdown
> Status: **in design.** This README describes what herdr-pets is meant to be.
> The project's north star lives in [GOAL.md](GOAL.md), the phased roadmap in
> [docs/PLAN.md](docs/PLAN.md); each phase's detailed design + plan land in
> `docs/superpowers/specs/`.
```

Leave the commented-out screenshot line (`<!-- ![herdr-pets...] -->`) in place — it's the maintainer's image slot.

- [ ] **Step 2: Replace the Install section**

Replace the entire existing `## Install` section (from `## Install` through the `herdr-pets requires **herdr ≥ 0.7.0**.` line) with:

```markdown
## Install

```sh
herdr plugin install vandemaelefelix/herdr-pets
```

That's it. herdr fetches a **prebuilt binary** for your platform (macOS on Apple
Silicon or Intel, Linux on x86-64 or arm64) — **no Rust toolchain required**. On
any other platform it falls back to building from source with `cargo`, so the
install still succeeds wherever Rust is available.

Requires **herdr ≥ 0.7.0**.

To pin a specific version, pass a tag: `herdr plugin install
vandemaelefelix/herdr-pets --ref v0.1.0`.

### From a local checkout (development)

```sh
herdr plugin link .
```

## Quickstart

After installing, start the watchdog once — it keeps a pets strip present in
every eligible tab:

```sh
herdr plugin action invoke herdr-pets start-pets-controller
```

(herdr fires no plugin-start hook, so the watchdog doesn't auto-start on a fresh
session — run this once per session, or from herdr's action picker.) For a
one-off strip in just the current tab, use the **`place-pets`** action instead.
See [Usage](#usage) for the difference between the two.
```

- [ ] **Step 3: Fix the License section**

Replace the final License section:

```markdown
## License

TBD.
```

with:

```markdown
## License

MIT — see [LICENSE](LICENSE).
```

- [ ] **Step 4: Verify the stale markers are gone**

Run: `! grep -nE "in design|License.*TBD|TBD\." README.md && echo "clean"`
Expected: prints `clean` (grep finds none of the stale phrases). Then eyeball the Install + Quickstart sections render correctly (nested code fences intact).

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs(packaging): rewrite install for one-line prebuilt install, add quickstart, fix license"
```

---

### Task 5: Final gate, decisions record, and release checklist

Runs the full gate, records the packaging decision in `docs/decisions.md` (superseding the earlier "deferred" note), and writes down the exact manual steps the maintainer runs to cut the release.

**Files:**
- Modify: `docs/decisions.md`

**Interfaces:**
- Consumes: everything from Tasks 1–4.

- [ ] **Step 1: Run the full gate**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS — fmt clean, no clippy warnings, all tests green (including the new `build_script` tests and the existing `manifest` tests).

- [ ] **Step 2: Append a decisions-log entry**

Add to the end of `docs/decisions.md`:

```markdown
## 2026-07-26 — Packaging: fetch-or-build install (ships the deferred Phase 4 item)

**Decision:** Ship the packaging/release that Phase 4 deferred (see the
2026-07-23 entry). `scripts/build.sh` now downloads a prebuilt binary for the
platform and only runs `cargo build --release` as a fallback, so installing
needs **no Rust toolchain** on the four published targets
(`aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`,
`aarch64-unknown-linux-gnu`). A tag-triggered `release.yml` cross-compiles and
publishes those binaries to a GitHub Release; `ci.yml` (fmt/clippy/test) stays
the PR gate.

**Chosen over:** source-build-only (fails the "easy for others" goal — needs
Rust) and prebuilt-only (drops the free robustness of the fallback). Fetch-or-
build gives "no toolchain for the common case" and "never hard-fails" from one
design. Spec: `docs/superpowers/specs/2026-07-26-packaging-and-install-design.md`.

**Risk accepted:** `aarch64-unknown-linux-gnu` is cross-compiled with the
`gcc-aarch64-linux-gnu` linker (no C deps in the tree, so this is safe). If that
matrix leg ever proves fragile, drop it — arm64-Linux users then hit the
source-build fallback, still a working install where Rust is present.

**Manual maintainer step:** cutting the tag/release is done by the human (repo
convention: no autonomous commits/pushes) — see the release checklist in the
plan.
```

- [ ] **Step 3: Commit**

```bash
git add docs/decisions.md
git commit -m "docs(packaging): record fetch-or-build packaging decision"
```

- [ ] **Step 4: Release checklist (MAINTAINER-RUN, after this branch merges to `main`)**

These are **not** run by the implementing agent — they require a push to a
real tag, which fires the release workflow. Do them after the PR merges:

```bash
# 1. Pre-tag: confirm the version agrees everywhere (all must print 0.1.0).
grep '^version' Cargo.toml herdr-plugin.toml
grep 'v0.1.0' tests/build_script.rs

# 2. Land the branch, then tag and push from main.
git checkout main && git pull
git tag v0.1.0
git push origin v0.1.0

# 3. Watch the Release workflow finish (4 build jobs + publish):
gh run watch

# 4. Verify the Release has all four assets:
gh release view v0.1.0

# 5. Optional pre-flight on a throwaway tag first (catches a broken matrix off
#    the real tag): push v0.1.0-rc.1, confirm the run is green, delete it.

# 6. End-to-end on a fresh machine / scratch context:
herdr plugin install vandemaelefelix/herdr-pets
herdr plugin log list --plugin herdr-pets   # expect "installed prebuilt binary"
```

Expected: the Release lists four `herdr-pets-<target>` assets; install pulls the
prebuilt binary (log says "installed prebuilt binary", not "building from
source") and the strip renders.

---

## Self-Review

**Spec coverage:**
- Spec §4.1 release workflow → Task 3. ✓
- Spec §4.2 fetch-or-build `build.sh` (version read, uname map, download+verify, fallback, POSIX, stderr note) → Task 1 (all present in the script + `--print-url` seam). ✓
- Spec §4.3 manifest tidy → Task 2. ✓
- Spec §4.4 README (drop "in design", rewrite Install, Quickstart, License=MIT) → Task 4. ✓
- Spec §4.5 cut release `v0.1.0` (manual) → Task 5 Step 4. ✓
- Spec §6 error handling (degrade, exit non-zero only if both fail) → Task 1 script logic. ✓
- Spec §7 testing (target-mapping seam, manifest still parses, workflow validates, e2e manual) → Task 1 tests, Task 2 Step 2, Task 3 Step 2, Task 5 Step 4. ✓
- Spec §8 risks (arm64-Linux droppable, screenshot slot kept, version-drift checklist) → Task 3 matrix note in decisions (Task 5 Step 2), Task 4 Step 1 (slot kept), Task 5 Step 4 (version check). ✓

**Placeholder scan:** No TBD/TODO/"handle edge cases" — every code and config step shows full content. ✓

**Type/name consistency:** Asset name `herdr-pets-<target>` is identical in `build.sh` (Task 1), `release.yml` upload/stage (Task 3), and the README (Task 4). The four target triples match across Task 1's `case`, Task 3's matrix, and the Global Constraints. Version `0.1.0` / tag `v0.1.0` consistent across manifest, `build.sh` URL, `tests/build_script.rs`, and the checklist. Action id `start-pets-controller` matches the manifest. ✓
