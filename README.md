# scrapmf — Safe, interactive archiver for social media galleries

> A friendly front-end for archiving your own galleries. `scrapmf` does not
> reimplement downloading — it securely drives a bundled, version-pinned
> [gallery-dl](https://github.com/mikf/gallery-dl).

[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-GPL--3.0-blue)](LICENSE)
[![CI](https://github.com/Scrap-MF/ScrapMF-Rust-CLI/actions/workflows/ci.yml/badge.svg)](https://github.com/Scrap-MF/ScrapMF-Rust-CLI/actions/workflows/ci.yml)

---

## What is scrapmf?

A single-binary CLI that archives content from social and media networks
(TikTok, Instagram, Twitter/X, VSCO, Facebook, Threads, …) that you have access
to — for backup, offline viewing, or curation.

- **You get one command.** It handles the backend, your cookies, your settings
  and the download report.
- **You get a menu.** Run it with no arguments for a guided interface.
- **Re-runs don't duplicate.** It remembers what it already downloaded.

`gallery-dl` (300+ sites) does the actual downloading. `scrapmf` wraps it so you
don't have to learn its flags.

---

## Install

Every method puts `scrapmf` in your `$PATH`.

**Linux / macOS (one-liner):**

```bash
curl -fsSL https://raw.githubusercontent.com/Scrap-MF/ScrapMF-Rust-CLI/main/install.sh | sh
```

Downloads the latest build for your architecture, verifies its checksum, and
installs to `~/.local/bin`. Pass a version tag (`install.sh v1.0.0`), an install
directory (2nd argument), or `--gnu` for the dynamically-linked build.

**With Cargo** (needs Rust 1.88+):

```bash
cargo binstall scrapmf                 # prebuilt binary
cargo install --path .                 # or build from this repo
```

**Windows (via WSL):**

scrapmf has no native Windows binary. Install it inside WSL using the Linux
one-liner above; from there the rest of this guide applies unchanged.

<details>
<summary><strong>Termux (Android)</strong></summary>

The static musl builds run natively on Android, no root required:

```bash
pkg install curl tar python
curl -fsSL https://raw.githubusercontent.com/Scrap-MF/ScrapMF-Rust-CLI/main/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"    # add this line to ~/.bashrc
```

</details>

Verify:

```bash
scrapmf --version
```

### Where it runs

| Platform | Status |
|---|---|
| Linux x86_64 / ARM64 / ARM32 | static musl + dynamic gnu |
| Linux RISC-V 64 / POWER 64 LE | dynamic gnu |
| Android (Termux) | static musl |
| Windows | via WSL (use the Linux build) |
| macOS | build from source with `cargo install --path .` |

---

## First run

`scrapmf` ships its **own pinned copy of gallery-dl**, separate from any system
installation. The first time you start the menu, it offers to set this up for
you — on x86_64 it downloads the official build (~23 MB) and verifies its
checksum; on other architectures it uses `pipx` or `pip`.

Confirm with <kbd>Enter</kbd> when it asks. Afterwards:

```bash
scrapmf doctor
```

shows something like:

```
gallery-dl 1.33.0-dev found [bundled (pinned)] pinned v1.33.0-dev
```

The bundled copy never updates on its own — new pins arrive with `scrapmf`
releases, so an upstream change can never break your setup mid-archive.

---

## Use it

### The menu (recommended)

```bash
scrapmf
```

A five-item menu, each entry showing live details on the right:

| Entry | What it does |
|---|---|
| **Download content** | Pick a site, then an account. Choose posts, reels, stories, albums — whatever that account has. |
| **Configuration** | Config file, profiles (one person → many accounts), per-site settings, cookie profiles. |
| **Plugins** | Toggle optional extras. |
| **Doctor** | Checks the backend, cookies and system. Start here if something fails. |
| **Exit** | Quit. |

**Keys**

| Key | Action |
|---|---|
| <kbd>↑</kbd> <kbd>↓</kbd> | move |
| <kbd>Enter</kbd> | confirm / select |
| <kbd>Esc</kbd> | back, or cancel |
| <kbd>Ctrl</kbd>+<kbd>C</kbd> | cancel now (< 1s, even mid-download) |

### The command line

For scripts, cron jobs, or CI:

```bash
# see what would be downloaded, without downloading
scrapmf scrape "https://www.tiktok.com/@sbtn_mf" --dry-run

# actually download
scrapmf scrape "https://www.instagram.com/sbtn_mf/" --output ~/pictures/sbtn_mf
```

| Flag | Meaning |
|---|---|
| `-o, --output <PATH>` | Where to put files (default `~/scrapmf`) |
| `--cookies <FILE>` | Cookies file in Netscape `cookies.txt` format |
| `--cookies-from-browser <NAME>` | Read cookies from `chrome`, `edge`, `brave`, `firefox` |
| `--preset <NAME>` | Apply a saved preset |
| `--dry-run` | List what would be downloaded, download nothing |
| `--no-archive` | Ignore the "already downloaded" memory for this run |
| `--profile-pic-only` | Threads: just the profile picture |

More options: `-v` for progress, `-vv` for debug detail. Exit codes are `0`
success, `1` error, `2` missing subcommand, `130` cancelled with <kbd>Ctrl</kbd>+<kbd>C</kbd>.

---

## Logged-in content

Most of what you probably want sits behind a login. Point `scrapmf` at your
cookies:

```bash
# read straight from a desktop browser
scrapmf scrape "https://www.instagram.com/sbtn_mf/" --cookies-from-browser brave
```

In the menu, **Configuration → cookie profiles** does the same thing and saves it
for next time. `scrapmf` stores cookies at `0600` and never logs them.

> **Android:** `--cookies-from-browser` cannot work in Termux — browsers keep
> cookie databases in app-private storage that Termux cannot read without root.
> That is an Android limit, not a scrapmf limitation. Export `cookies.txt` with
> a Firefox extension instead, copy it into `~/.config/scrapmf/cookies/`, and
> use `--cookies`.

### Android / Termux cookie workflow

The companion Firefox extension **ElDevoraCucas**
([Scrap-MF/ElDevoraCucasMF](https://github.com/Scrap-MF/ElDevoraCucasMF))
exports a Netscape `cookies.txt` for Instagram, TikTok, Twitter/X, VSCO,
Threads and Facebook straight from a logged-in Firefox session.

1. Install the extension in **Firefox for Android**. The simplest route is
   Firefox Nightly with `xpinstall.signatures.required` set to `false`: copy
   `ElDevoraCucasMF.xpi` to the phone (USB, cloud, email), open it in Firefox,
   and accept the warnings. AMO listing is coming and will not need this step.
2. Log in to the sites you want inside Firefox.
3. Open the extension popup, tick the networks, and download `cookies.txt`.
4. Copy the file into Termux's config dir:

   ```bash
   termux-setup-storage   # once per fresh install
   cp ~/storage/downloads/cookies.txt ~/.config/scrapmf/cookies/android.txt
   scrapmf scrape "https://www.instagram.com/sbtn_mf/" \
     --cookies ~/.config/scrapmf/cookies/android.txt
   ```

Firefox writes downloads to `~/storage/downloads`, which Termux reaches through
the symlink that `termux-setup-storage` creates. scrapmf reads the file you hand
it and does not refresh it — re-export when the session expires.

> Cookie files are account credentials. Never commit or share them.

---

## What happens after a download

At the end of every run you get a short report:

```
✔ 31 files · quality: 1080x1920 avc1 ×28, 720x1280 hvc1 ×3
```

It flags incomplete downloads and stray `.part` files. If a site's anti-bot
system quietly swallowed some posts, it tells you how many and how to fix it —
usually by refreshing your browser session.

**Re-runs skip what you already have.** Every downloaded item is recorded in a
plain-text file you own:

```
~/.config/scrapmf/archive/instagram/sbtn_mf.jsonl
```

Run it again next month and only the new posts are fetched. Copy the `archive/`
folder between machines and the memory comes with it. Turn it off with
`--no-archive`.

---

## When something goes wrong

Run `scrapmf doctor` first — it reports the backend version, cookie
availability and system details in one screen.

| Symptom | Fix |
|---|---|
| `gallery-dl not found` | Restart `scrapmf` and accept the setup offer, or run `scrapmf setup` |
| Posts are private / empty | Log in to the site in your browser, then re-capture cookies |
| `HTTP Error 429` (rate limited) | `scrapmf` waits and retries automatically (60s, then 120s). Slower than that: add a preset with a higher rate limit. |
| Interrupted download | Just run it again — completed files are kept and skipped |
| Interactive menu renders wrong | Use a terminal with true colour support |
| Instagram reels/stories fail (`NotFoundError`) | Refresh cookies (Configuration → Cookie profiles → Refresh); posts work but reels/stories need a fresher session. Try `https://www.instagram.com/id:<ds_user_id>/reels/` or export fresh cookies with ElDevoraCucas. If it persists, gallery-dl 1.32.17 has an upstream bug fixed only in dev: install dev gallery-dl and set `SCRAPMF_GALLERY_DL` or `backend.gallery_dl_path` (see docs). |

Config lives in `~/.config/scrapmf/config.toml` (created on first run):

```toml
[general]
output_dir = "~/scrapmf"
```

```bash
scrapmf config path    # where the config file is
scrapmf config edit    # open it in $EDITOR
scrapmf config list    # print the current settings
```

---

## Scope & disclaimer

- **Personal archiving only.** Use this on content "you have the right" to access.
  You are responsible for following each site's Terms of Service and local
  copyright law.
- **No paywall bypass.** `scrapmf` implements no extractors of its own; it calls
  gallery-dl exactly as upstream ships it.
- **Not affiliated** with any of the supported sites or with gallery-dl.

---

## License

`GPL-3.0-only` — see [`LICENSE`](LICENSE).

## Acknowledgements

Built on [gallery-dl](https://github.com/mikf/gallery-dl) by mikf and
contributors. Not affiliated.

---

Created by **mfapplications** ([@sbtn_mf](https://www.instagram.com/sbtn_mf)).