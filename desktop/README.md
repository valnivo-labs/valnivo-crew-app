# Valnivo Crew for macOS and Windows

The Console as an app: one window on <https://console.valnivo.eu/ui/>, the same Console as in the browser, signed in with the
same account. It is a [Tauri 2](https://tauri.app) shell (Rust, the system's own web view: WebKit on macOS, WebView2 on Windows),
so it is small. The page updates the next time the window loads. From 0.10.0 the window can update itself: Settings shows the installed version, Check for updates, and Update now. Later waits 24 hours before asking again. The check also runs in the background about every 6 hours and does not block launch.

People on 0.9.0 install 0.10.0 once from the downloads page (that build has no updater). After that, Update now downloads the signed package, checks the signature, installs it and restarts the app. If that fails, the same downloads page is the recovery path. Linux is not a target. The updater signature is not an Apple or Windows certificate, so the first open can still show a system warning.

What the app adds over the browser tab: its own window and Dock / taskbar icon, remembered size and place, one copy at a time
(opening it again brings the window forward), and links to other sites open in your normal browser. The page may show a notification, set the Dock badge, read the app version, and install a signed update. No other command is exposed.

## Give control (from 0.11.0)

When you share your **entire screen** in a chat and press **Give control**, the chat's agent clicks and types on your computer
for the time you picked. A web page cannot do that (no browser lets a site move your mouse), so the app does it: while it runs it
answers the chat on `127.0.0.1:47614` (only on this computer, only for the Console's own pages), the same small protocol as the
separate `desktop-helper` that browser users install. Push the mouse into the **top-left corner** to stop it at once, or press
**Take back control**.

- **macOS:** the first time, macOS asks to allow *Valnivo Crew* under **System Settings → Privacy & Security → Accessibility**.
  Until the app is signed with an Apple Developer ID, macOS may ask again after an update (the ad-hoc signature changes).
- **Windows:** nothing to allow. It cannot click into windows that run as administrator.
- If the separate helper already runs, the app leaves the port to it.

### Two input modes (from 0.14.0, `src-tauri/src/input_guard.rs`)

- **You move, the agent waits** (always, the default). Any real mouse movement, click or key press of yours pauses the agent
  until you have left the mouse and keyboard alone for 4 seconds. The agent's own clicks and keys never count (Windows: the
  "injected" flag of the input hooks; macOS: the app marks its own events). Nothing of yours is ever blocked in this mode. The
  chat asks again every 2 seconds for up to 15 seconds per action, then tells the agent it is waiting for you.
- **Hands-off** (opt-in: the checkbox beside Give control, remembered like the duration). While control is on, your own mouse
  and keyboard are paused so nothing gets in the agent's way; only the agent's input goes through. The red banner says
  *"Hands-off: your mouse and keyboard are paused. Press Esc 3 times to take back control."* Ways out, always on:
  1. **Esc 3 times within 1.5 seconds**: hands-off and control end at once; the chat shows "You took back control".
  2. **A minute without an agent action**: your mouse and keyboard come back on their own (for the rest of that grant) and the
     banner says so.
  3. **Control ends** (Take back control, Stop sharing, the end time), **the app quits or crashes**: the hooks belong to the app's
     process and go with it.
  4. **The system's own escapes are never touched**: Ctrl+Alt+Del on Windows cannot be hooked at all; Ctrl+Shift+Esc (Windows)
     and Cmd+Option+Esc (macOS) pass; key releases and Shift/Ctrl (macOS: all modifier keys) always pass, so nothing stays held down.

  Windows uses low-level hooks (`WH_MOUSE_LL` / `WH_KEYBOARD_LL`) on their own thread; macOS uses an event tap, which needs the
  same Accessibility permission. Without it (or with an older page) hands-off falls back to the default mode, and the chat says
  so. Trackpad gestures that are not clicks, moves or scrolls (three-finger swipes) are not paused on macOS. The hooks are only
  installed while control is on.

  The Windows code is type-checked on a Mac with `RUSTFLAGS="--cfg win_check" cargo check`; it is built and run only by the
  Windows job of the public build.

## Getting the apps

The apps are built in the **public** repository [valnivo-labs/valnivo-crew-app](https://github.com/valnivo-labs/valnivo-crew-app),
where GitHub Actions minutes for macOS and Windows are free (this private repo's Actions billing does not cover them).
Download them from its [Releases](https://github.com/valnivo-labs/valnivo-crew-app/releases).

This folder stays the source. `scripts/desktop-publish.sh` copies `desktop/` there, and `desktop/ci/desktop.yml` into the public repo's workflow. Everything in this
folder becomes public, so keep it to the app shell, never keys or server code.

```
scripts/desktop-publish.sh            # sync only
scripts/desktop-publish.sh --build    # sync and build by hand: the run's artifacts
scripts/desktop-publish.sh --tag      # sync and tag desktop-v<package.json version>: build and publish a Release
```

Each run's artifacts are:

- `valnivo-console-macos`: `Valnivo Crew_<version>_universal.dmg` (Apple silicon and Intel, macOS 11 or later) and `Valnivo Crew.app.tar.gz` plus its signature (what the updater installs)
- `valnivo-console-windows`: `Valnivo Crew_<version>_x64-setup.exe` (Windows 10 and 11; installs for the current user) plus its signature
- `latest.json` on the release, which the app reads

The build refuses to run until the public repository has the updater signing key in its Actions secrets (`TAURI_SIGNING_PRIVATE_KEY`, and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` only if that key has a password). The private key is not in this folder. The matching public key is in `src-tauri/tauri.conf.json`. Losing the private key means installed copies cannot take a later update.

## Checking a download

Every released file carries a build provenance attestation (SLSA Build L2): GitHub signs that the file came from this
repository's `Desktop apps` workflow, at that commit. Check one before installing it:

```
gh attestation verify "Valnivo-Crew-macOS.dmg" -R valnivo-labs/valnivo-crew-app
```

The workflow lives only in the public repository (`.github/workflows/desktop.yml`); its outside actions are pinned to exact commits.

## Not signed yet

Until the owner's certificates are added, both systems warn on first open:

- **macOS** (ad-hoc signed): "Apple could not verify…". Press **Done**, then **System Settings → Privacy & Security → Open
  Anyway**. Or, once: `xattr -dr com.apple.quarantine "/Applications/Valnivo Crew.app"`.
- **Windows** (unsigned): SmartScreen. **More info → Run anyway**.

Signing needs the owner's Apple Developer ID (with notarisation) and a Windows code-signing certificate; both are added as
secrets of the public build repository and its workflow then signs. Nothing else changes.

## Windows title bar

The page background is `#0a0b0d` and the text is `#e9e9ea`. On Windows 11 the native caption, its text and its border are set
to those colors through the Desktop Window Manager. If Windows refuses a color, the window stays open and only the dark caption
is requested. Windows 10 builds that predate the color attributes get that dark caption, not the exact page color. macOS is
unchanged. A compile on this desk is not a look at the real window.


## Building on your own machine

Needs Node 20+ and Rust (`brew install rust` on macOS; rustup on Windows).

```
cd desktop
npm ci
npx tauri build                 # this machine's app and installer
npx tauri dev                   # a window for trying changes
```

The icon is `icon/crew.svg` (drawn at 1024 px as `icon/crew-1024.png`; `npm run icons` makes every size from it). The address is `HOME` in `src-tauri/src/main.rs`; pages on
valnivo.eu hosts stay in the window, anything else opens in the browser.
