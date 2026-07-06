# Installation

Argus is a **lightweight, AI-native, local-first** desktop app for macOS and Windows.
This page covers how to download and install it.

## macOS

1. Download the latest `.dmg` from the [Download](/download) page (or the
   [Releases](https://github.com/chenwen245299/Argus/releases) page) and install it.
2. I can't afford an Apple Developer account, so the app isn't code-signed and macOS blocks
   it on first open. To get around this, just clear the quarantine flag by running the
   following in Terminal:

   ```bash
   xattr -cr /Applications/Argus.app
   ```

3. Launch Argus from Applications.

## Windows

Download the latest installer (`.msi` / `.exe`) from the [Download](/download) page and
run it.

## Building from source

Requires Node.js 22+ and the Rust stable toolchain.

```bash
npm install          # install deps (also copies Vditor assets)
npm run tauri dev    # run the desktop app in dev mode
npm run tauri build  # build a production installer for your platform
```

## Next

Installed? Continue to [Quick Start](/guide/quick-start).
