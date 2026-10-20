# umu-run in Rust: no helper binaries left

**Done (2026-10-19):** `umu-run` (a Python zipapp that needed a host `python3`) is replaced by
`core/src/compat/umu/`, a port of umu-launcher 1.4.4 for the one way Hyper used it. The Linux
builds bundle no external program any more: `externalBin`, `download-helpers.mjs`, its SHA-256 table
and the `--verify` step before every build are gone, `build-linux.sh` downloads nothing, and
umu-launcher is an `origin` (ported code) entry in `gen-licenses.mjs`. Same approach as the Epic and
GOG ports (`docs/legendary-rust.md`): compatible files on disk, offline tests, real-game checks on
the Deck before the port is called final.

## Why

- No host Python: SteamOS has one, other distros / immutable systems may not; the Deck edition is
  self-contained now.
- One less binary to pin, verify and ship; a launch no longer unpacks a Python runtime first.
- Progress and errors as typed events: the UI shows what a first start is downloading and how far it
  is instead of a bare "Preparing".

## The cost to keep in mind

umu follows what Steam does for Proton games (environment, runtime layout, pressure-vessel
arguments) and what UMU-Proton expects (`GAMEID` / `STORE` for protonfixes). When Valve or
Open-Wine-Components change something, umu is fixed first. Whoever owns this code must follow
`Open-Wine-Components/umu-launcher` (`umu/umu_run.py`, `umu_runtime.py`, `umu_proton.py`,
`umu_consts.py`) and port fixes.

## What umu did for us, and where it lives now

| umu-launcher | module | notes |
|---|---|---|
| folders (`umu_consts.py`: `UMU_LOCAL`, `UMU_CACHE`, `STEAM_COMPAT`, `UMU_COMPAT`, `XDG_*`, `UMU_FOLDERS_PATH`) | `umu/mod.rs::Dirs` | same paths, so a runtime / Proton set up by umu is reused |
| `get_umu_proton` with `PROTONPATH` unset: GitHub latest release, `.sha512sum`, resumable download to `~/.cache/umu/<tar>.parts`, install into `compatibilitytools.d`, fallback to the newest installed UMU-Proton | `umu/proton.rs` | the GitHub check is remembered for an hour (`cache/umu-proton-check.json`); umu asks on every launch |
| `resolve_runtime` / `CompatLayer.required_runtime` (`toolmanifest.vdf` → `require_tool_appid`) | `umu/runtime.rs::required_by`, `umu/vdf.rs` | soldier / sniper / steamrt4 (x86-64 only) |
| `setup_umu`: `latest-public-beta.txt`, `SHA256SUMS`, `BUILD_ID.txt`, resumable download, unpack, `renameat2(RENAME_EXCHANGE)` swap, `pv-verify`, `.installed.ok`, `umu` → `_v2-entry-point`, `VERSIONS.txt` update check, restore of broken installs | `umu/runtime.rs` | same lock files (`umu.lock`, `compatibilitytools.d.lock`, `pfx.lock`), polled so a cancel works; a failed `pv-verify` leaves `.hyper-unverified` instead of a re-download per launch |
| `create_shim` | `umu/mod.rs::SHIM` | byte-for-byte umu's script |
| `setup_pfx` | `umu/mod.rs::setup_prefix_async` | `pfx` → `.`, `tracked_files`, `steamuser` ↔ Unix user link |
| `set_env` + `enable_steam_game_drive` | `umu/mod.rs::launch` | every variable umu sets, including the empty ones; `ldconfig -p` for `STEAM_RUNTIME_LIBRARY_PATH` |
| `build_command` (`CompatLayer.command`) | `umu/mod.rs::command` | `<rt>/umu --verb=waitforexitandrun -- <shim> <proton>/proton waitforexitandrun <exe> …` |
| `run_command` (process group, signal forwarding) | `games.rs` / `process.rs` | the game runner already starts the tree in its own group and kills the group |
| `umu-run ""` (create the prefix) | `compat/mod.rs::Compat::prepare` | still a separate "Preparing" step |

Not ported (unused by Hyper): TOML launch configs (`umu_plugins.py`), winetricks verbs, every
other `PROTONPATH` (GE-Proton, `*-Latest` with signed delta patches, `umu-sniper` passthrough tools
for native Linux games), `UMU_NO_RUNTIME` / `UMU_NO_PROTON` / `UMU_RUNTIME_UPDATE` (forwarded to Proton for parity, not acted on), re-entering a running container
(`UMU_CONTAINER_NSENTER`), the Flatpak + gamescope window tagging (`run_in_steammode`), zenity
progress windows, the arm64 runtime.

## Ground rules

- **Same files on disk as umu.** Runtime in `~/.local/share/umu/<variant>` with umu's markers and
  link, UMU-Proton in `~/.local/share/Steam/compatibilitytools.d/<name>`, partial downloads in
  `~/.cache/umu` under umu's `.parts` names (a download umu started is resumed by us and the other
  way round), prefixes unchanged. Switching back to umu at any point is safe.
- **Offline behaviour like umu.** Proton / runtime present → launch without the network; nothing
  installed → a clear error that the first start needs a connection. A failed update keeps the
  installed version.
- **Archives are verified** against the digest published next to them (SHA-512 for Proton, SHA-256
  for the runtime) before they are unpacked; a mismatch deletes the download.
- **Cancel** stops a download (the partial file stays), a wait for another launcher's lock, and kills a running prefix creation.
- **Locks never block for good.** umu's lock files are taken with `flock(LOCK_EX | LOCK_NB)` polled every
  250 ms (`umu::lock`, `fsutil::try_lock`): while umu / Steam / Heroic holds one, the UI shows "Waiting
  for another launcher to finish" and a cancel ends the wait (umu itself blocks in `flock`).
- **A runtime that fails `pv-verify` is not fetched again on every launch.** umu writes `.installed.ok`
  only after a passing check and re-installs without it; we use the runtime anyway (like umu) and write
  `.hyper-unverified` with its version from `VERSIONS.txt` and count it as installed, so the normal
  `VERSIONS.txt` comparison decides: the same version as the remote is kept (warned about once), a
  newer build replaces it (the stamp goes with the old files); missing files still trigger the restore (`install(force = true)`, which
  ignores "another launcher finished it"). Interrupted unpacks (`.hyper-*` folders) are cleaned under the
  lock; an archive whose content check fails is deleted, not resumed.
- **umu database lookups** (`cache/umu-ids.json`): a found id is kept for good (a plain string, the old
  layout); "not in the database" is stored as `{"id": "umu-default", "checkedAt": <unix s>}` and asked
  again after 7 days; offline (`net::reachable`) nothing is asked; a network error is not cached.
- **Helper output is filtered.** `run_in_prefix` (setup scripts, `start link2ea://…` with Epic's exchange
  code) pipes stdout / stderr through `games::log_filtered` (`redact_text`) into `-setup.log`, like the
  play path. Remote names are checked: GitHub asset names must be one plain file name starting with
  `UMU-Proton`, digests must have the exact SHA-256 / SHA-512 length, small text files are capped at 1 MiB
  / 15 s, VDF nesting at 64, and redirects are followed to https only.
- **Uninstall** removes a prefix only when it looks like one (`pfx`, `tracked_files` or `drive_c`) and
  lies inside the prefixes folder (both canonicalised).

## Tests

Offline unit tests in each module: the VDF reader, release-asset selection, digest-list parsing,
`VERSIONS.txt` / version ordering, archive names, the command line and environment against fake
tool folders (relative exes against the launch folder, bare names passed through), the prefix layout
(Unix), tar.gz extraction, the folder swap, lock polling, stale temp folders, `ldconfig -p` parsing, the
umu-id cache layout, the prefix-removal checks, asset-name and digest-length checks, the VDF depth cap.

Real-game checks (owner, Steam Deck): first start with nothing installed (both downloads, prefix
creation, the game runs), second start (no downloads, launch time), start with the network off, a
prefix made by umu before the port, cancel during a download and the resume on the next start.
