# Epic in Rust: replacing legendary

**Done (2026-10-19):** all three stages are in and were checked on real games (install, cloud saves);
`legendary` is no longer bundled (`externalBin`, `download-helpers.mjs`, the patch and
`build-legendary.ps1` are gone), and `stores::legendary` has no helper path left. Encrypted manifests and
chunks (manifest version 22+/24+) are supported: the keys come with the manifest record (`secrets`). What follows is the plan
as it was written, kept for the ground rules and the list of what was not ported.

Owner decision (2026-10-18): replace the bundled `legendary` (PyInstaller exe) with our own Rust code in
`core`, in stages. legendary stays bundled as the fallback until the last stage has run on real libraries.

## Why

- Speed: every legendary call unpacks a Python runtime first (0.5–1.5 s per call, more on the Deck).
- Security: tokens live only in memory and the vault; no plaintext `user.json` during calls or downloads.
- No helper processes to tie to the app, hash-pin or kill.
- Progress, errors and cancel as typed events instead of parsing console text.

## The cost to keep in mind

Epic publishes no API for third-party launchers. legendary is community reverse engineering of the Epic Games
Launcher; when Epic changes something, legendary (and Heroic's fork) get fixed first. Whoever owns this code
must follow both repositories and port fixes: watch `derrod/legendary` and `Heroic-Games-Launcher/legendary`
(commits touching `api/egs.py`, `models/manifest.py`, `downloader/`, `core.py` launch / save code).

## What we use today (and only this gets ported)

| legendary command | where | ported in |
|---|---|---|
| `auth --code` | login | done (`legendary::exchange_code`, 2026-10-18) |
| `list --third-party` | library refresh | stage 1 |
| `info --json` | install sizes | stage 1 |
| `launch --dry-run --json [--offline]` | launch | stage 1 |
| `import` | re-attach installs | stage 1 |
| `uninstall` | uninstall | stage 1 |
| `sync-saves` | cloud saves | stage 2 |
| `install` / `update` / `repair` | downloads | stage 3 |

Not ported (unused by Hyper): EGL sync, EOS overlay, Ubisoft / Origin activation, achievements, aliases,
`cleanup`, `move` (we move ourselves), CrossOver / Wine setup (we use umu).

## Ground rules

- **Same files on disk as legendary** (`installed.json`, `metadata/*.json`, `assets.json`, `manifests/*.manifest`
  in `legendaryConfig/legendary`). Existing installs keep working, and switching back to legendary at any point
  is safe. `user.json` is the one exception: it stays in the vault and is only materialised for legendary calls.
- **One switch per stage**: `AppSettings.epic_native: { library, saves, downloads }` (default on once a stage
  ships, off = legendary). A native failure in a read-only path (library, info) falls back to legendary
  automatically and logs it; write paths (downloads, saves) do not silently fall back.
- **New module** `core/src/stores/epic/`: `api.rs` (HTTP, token refresh), `manifest.rs`, `chunk.rs`,
  `download/` (planner, workers, writer), `saves.rs`, `launch.rs`. `stores/legendary/mod.rs` keeps the
  `StoreManager` impl and picks native or helper per call.
- **Tests against legendary**: every parser gets golden tests on real manifests / API answers saved from
  legendary (`core/tests/data/epic/`, scrubbed of tokens). Stage 1 and 3 are checked by running both
  implementations on the owner's library and comparing outputs (debug command `epic-compare`).

## Stage 1: session, library, launch (≈ 1–2 weeks) — code in, awaiting real-library checks (2026-10-19)

Implemented in `core/src/stores/epic/` (`api`, `session`, `library`, `manifest`, `launch`, `installed`),
switched on by `epic_native.library` / `epic_native.launch` (default on). Not done from the list below:
the `epic-compare` debug command (compare by diffing `legendaryConfig/legendary` after a run with each
implementation for now), `version.json` is only refreshed daily like legendary. Encrypted manifests (v22+
with the encryption flag) are refused with `NotImplemented` and handed to legendary.

1. `api.rs`: client with the OAuth client from `version.json` (as `oauth_client` does now), token refresh
   (`grant_type=refresh_token`) under the vault lease, 429 retry with backoff (legendary's patch behaviour),
   `verify` / re-login detection.
2. Library: `launcher/api/public/assets/{platform}?label=Live` (assets), `library-service …/library/api/public/items`
   (owned items incl. third-party), `catalog …/bulk/items` (metadata, batched), write `assets.json` and
   `metadata/<app>.json` like legendary. DLC mapping and `CanRunOffline` / `CloudSaveFolder` attributes come from
   the metadata as today (`library.rs` already reads these files).
3. Install info: fetch the game manifest (`…/assets/v2/platform/{p}/namespace/{ns}/catalogItem/{id}/app/{app}/label/Live`,
   then the CDN manifest URL), parse it (stage 3's parser, header + meta + sizes only at first) for download
   and disk sizes.
4. Launch: build `egl_parameters` ourselves: `-AUTH_LOGIN=unused -AUTH_PASSWORD=<exchange code>
   -AUTH_TYPE=exchangecode -epicapp -epicenv=Prod -EpicPortal -epicusername -epicuserid -epiclocale
   -epicsandboxid [-epicdeploymentid]`; exchange code from `account/api/oauth/exchange`; ownership token
   (`-epicovt=`) for games that require it (`ecommerceintegration …/ownershipToken`), written to a private temp
   file and deleted after the game exits; offline mode = no tokens. Executable / working dir / args from
   `installed.json`.
5. Import and uninstall: read the manifest from the game folder / our cache, check files, write / remove the
   `installed.json` entry; uninstall deletes only manifest-listed files, then `remove_game_dir` as today.

Done when: library, sizes and launch match legendary on the owner's Epic library (compare command), launch
time without the helper is measured, offline launch works.

## Stage 2 (`epic/saves.rs`) and stage 3 (`epic/download/`) — code in (2026-10-19), awaiting real-game checks

Both are on by default (`epic_native.saves` / `.downloads`). Tested offline: chunk and manifest round trips,
the planner (fresh install, update with chunk reuse, resume file, no-cache mode), verification, and an
end-to-end download from a local HTTP server (assembly, resume, repair of a damaged file, cancel). Not
ported on purpose: delta manifests (rarely offered; an update then downloads the changed chunks in full),
install tags / SDL (we never passed them), EGL sync, prerequisite installers (legendary skipped them with
`-y` too). Memory for chunks waiting to be written is capped at 1 GiB; beyond that a shared chunk is fetched
again instead of cached.

## Stage 2: cloud saves (≈ 1 week)

`datastorage …/api/v1/access/egstore/savesync/{account}/{app}/`: list remote saves (manifests), download the
newest (save manifests + chunks, same format as stage 3), upload = chunk the save folder, write a save manifest,
`create_game_cloud_saves` for upload URLs. Keep `saves::check_save_dir` and the pending-upload queue
(`pending-saves.json`) unchanged; only the transport changes. Port `utils/savegame_helper.py` closely.

Done when: download / upload / force both ways match legendary on 3 games with cloud saves, including a
conflict and an empty remote.

## Stage 3: install / update / repair (≈ 3–5 weeks, the big one)

1. `manifest.rs`: binary manifest (magic `0x44BEC00C`, zlib body): meta, chunk data list (CDL), file list (FML),
   custom fields; JSON manifests for old games. Encrypted manifests (`EncryptedData`, artifact-service tickets)
   only if a game in the owner's library needs them; otherwise refuse with a clear error and use legendary.
2. `chunk.rs`: chunk files (magic `0xB1FE3AA2`, zlib, SHA-1 / rolling hash check).
3. Planner (port of `downloader/mp/manager.py` analysis): diff old vs new manifest, reuse unchanged files and
   chunks, order work to keep the chunk cache small, disk-space check, resume file compatible with legendary's.
4. Workers on tokio: N parallel CDN downloads (base URLs from the manifest response, failover), decompress +
   verify per chunk, bounded memory (`max_shared_memory` like legendary), writer that assembles files in place;
   progress as `GameStatus` events (bytes, speed, ETA) instead of parsing text.
5. Repair / verify: SHA-1 of every file against the manifest, redownload broken ones.
6. DLC installs into the base game, SDL (selective downloads) if a game in the library uses it.

Done when: fresh install, update across 2 versions, repair of a damaged file and resume after a kill all
produce byte-identical files to legendary on at least 3 games of different sizes (one > 50 GB); download speed
at least legendary's; memory stays bounded.

## After stage 3

Drop legendary from the bundle (`externalBin`, `download-helpers.mjs`, `patched-helpers.json`, the patch and
`build-legendary.ps1`), keep the compare command for a release or two, update CLAUDE.md and the licences
(legendary is GPL-3.0 like us; ported code keeps an attribution note in `gen-licenses.mjs` `manual`).

## Quick win before stage 1

Build legendary as PyInstaller `onedir` instead of `onefile` (no unpacking on every call). Needs
`externalBin` → bundled folder + resolving the exe inside it; measurable in a day, worth it only if stage 1
is weeks away.
