# Platform notes

What differs on each system: requirements, known limits, where files live, and what a build needs. The [README](../README.md) has the short version.

## macOS

Working runs on macOS 13 or newer, on Apple silicon and Intel. Notifications and the Dock badge need `Working.app`, not a bare binary. The Dock number counts unread chats and, while Mail is on in Settings, Features, unread inbox threads.

Closing the main window keeps Working running. A click on the Dock icon opens it again; while a call window is open macOS brings that window forward instead, so the call window's top bar has an Open Working button while the main window is closed, and Working in the Window menu (`Cmd+0`) opens it too.

## Linux

### Requirements

The AppImage uses these system libraries rather than carrying its own:

- GTK 3, WebKitGTK 4.1, and libsoup 3 (on Debian and Ubuntu `libgtk-3-0`, `libwebkit2gtk-4.1-0`, `libsoup-3.0-0`; on Fedora `gtk3`, `webkit2gtk4.1`, `libsoup3`)
- An X11 session, or XWayland under Wayland
- A glibc at least as new as Ubuntu 24.04's, where the release is built

### Apps menu

The first time the AppImage runs, a card in the window asks to add Working to your apps menu.

- **Add** writes `~/.local/share/applications/working.desktop`, which runs the AppImage where it is, and the icon `~/.local/share/icons/hicolor/512x512/apps/working.png` (under `$XDG_DATA_HOME` when it is set). The dock and the launcher then show Working's icon and name, the open window included. Each start rewrites the entry when the AppImage has moved or been replaced.
- **No thanks** writes nothing, and the app does not ask again.
- **Settings › Appearance › Show in apps menu** changes the answer: off removes those two files, and only those, and the dock goes back to a generic icon and the name `working`.

Working neither asks nor writes under the AppImage opt-outs (a running `appimaged`, a `no_desktopintegration` file in `$XDG_DATA_HOME/appimagekit`, `/usr/share/appimagekit`, or `/etc/appimagekit`, or a non-empty `DESKTOPINTEGRATION` variable), or when a desktop entry for the AppImage that it did not write is already there, such as one from AppImageLauncher or Gear Lever.

### Meet

Meet opens in your default browser: joining from a meeting card, the Meet view, a conversation's call button, or a `https://meet.google.com/...` link given to `working` on the command line all hand the link to the browser. WebKitGTK as Fedora builds it has no WebRTC (`RTCPeerConnection` is missing even with WebRTC turned on), and the webview refuses camera and microphone requests, so a call cannot run inside the app. The call button still posts the meeting in the chat.

### Notifications

Notifications for messages, new mail, and event reminders go to the desktop's notification server over D-Bus. The freedesktop specification groups nothing, so they are not grouped by conversation. The unread count goes out as the launcher entry signal on `working.desktop`, which KDE Plasma's task manager, Ubuntu Dock, Dash to Dock, and Plank show on Working's launcher; it needs the apps menu entry above, and other desktops show no count.

### Emoji

Working draws emoji with its own copy of Google's Noto Color Emoji on every distribution. GPUI's Linux text stack cannot draw the COLRv1 color emoji font some distributions install (Fedora's `Noto-COLRv1.ttf`), so emoji would otherwise be blank there ([zed#42255](https://github.com/zed-industries/zed/issues/42255), [swash#21](https://github.com/dfrg/swash/issues/21)). The system emoji font is not used and does not need replacing.

### Known limits

- In the composer, an emoji the interface font also has, such as `❤️`, shows as that font's black glyph; the sent message shows it in color.
- In the media viewer, a video can stay black while its sound plays. This was seen in a Fedora 44 virtual machine whose GStreamer had the needed demuxers and decoders (Matroska, VP8, VP9, AV1, H.264), so missing codec packages are not the cause there. The cause is still open: the most likely one is that VM's VA-API video decoder, which isn't confirmed. Audio, PDF, and text files open normally.
- With the window maximized, a window button can keep its hover color after the pointer leaves the window through the top edge, until the pointer comes back in ([gpui-kit#3987](https://github.com/longbridge/gpui-kit/issues/3987)).
- The AppImage updates itself when it sits in a folder you can write to; a copy installed any other way, or in a folder you cannot write to, does not.

## Windows

The installer sets Working up for your user only, with no administrator rights, and installs the Microsoft WebView2 runtime if it is missing. It is not code-signed, so SmartScreen warns before it runs: choose **More info**, then **Run anyway**.

Notifications show only for the installed app, whose Start Menu shortcut carries the app's identity.

There is no unread count: Windows has none on a taskbar icon.

Updates keep your shortcuts as they are: the Start Menu shortcut is renewed, and a Desktop shortcut you deleted does not come back.

### Known limits

- In Mail, an email scrolled partly out of the reading pane keeps drawing past the pane, over the thread's subject line, the Mail toolbar, and the window's top bar. gpui-webview does not clip a webview to a scrolling pane on Windows.

## Mail

Each email's HTML renders in the system web view: WKWebView on macOS, WebKitGTK on Linux, and WebView2 on Windows. The page runs none of the email's scripts, frames, or forms, and its links open in the default browser. Notifications for new mail and the unread count follow each system's notes above, and on Windows an email can draw past the reading pane (see Windows, Known limits).

## Shortcuts

The README writes shortcuts the macOS way. On Linux and Windows, Ctrl takes the place of Cmd, with these exceptions:

| Action | Linux and Windows |
|---|---|
| Next or previous conversation | `Alt+Down`, `Alt+Up` |
| Newest unread conversation | `Shift+Alt+Down` |
| Open or close the sidebar | `Ctrl+Shift+S` |

Settings and the app's menus show each shortcut the way your system writes it.

## Where Working keeps its files

Settings, the session, and the MCP token (`settings.json`, `cookies.json`, `mcp-token`) live in one folder, the cache in another. `session.log` in the settings folder gets one line each time Google asks you to sign in again: when, and how long before that the session was last saved.

| System | Settings and session | Cache |
|---|---|---|
| macOS | `~/.config/working` | `~/Library/Caches/working` |
| Linux | `$XDG_CONFIG_HOME/working` (`~/.config/working`) | `$XDG_CACHE_HOME/working` (`~/.cache/working`) |
| Windows | `%APPDATA%\working` | `%LOCALAPPDATA%\dev.felipefdl.working\cache` |

On Windows, the web views keep their data in `%LOCALAPPDATA%\dev.felipefdl.working\WebView2`, and an update's installer waits in `%LOCALAPPDATA%\dev.felipefdl.working\updates` until the next launch removes it.

## Build prerequisites

Every system needs Rust 1.100.0 or newer and `protoc` (the Protocol Buffers compiler).

- **macOS:** [just](https://github.com/casey/just), Xcode (its `actool` compiles the app icon), and `brew install protobuf`. With an Apple Development certificate in your keychain, local builds keep camera and microphone permissions across rebuilds; without one, macOS asks again after every build.
- **Linux:** [just](https://github.com/casey/just) and the development packages. On Debian and Ubuntu:

  ```bash
  sudo apt install libgtk-3-dev libwebkit2gtk-4.1-dev libsoup-3.0-dev libxkbcommon-x11-dev libx11-xcb-dev \
    libxcb1-dev libwayland-dev libfontconfig-dev libasound2-dev libvulkan-dev libdbus-1-dev protobuf-compiler \
    clang pkg-config
  ```

  `./scripts/package-linux.sh` builds the AppImage into `target/dist`.

- **Windows:** the MSVC Rust toolchain with the Visual Studio Build Tools (C++ and the Windows SDK), `protoc` on the `PATH`, and on Arm64 clang on the `PATH` too. The `just` recipes are for macOS and Linux; on Windows, run the app with `cargo run --release -p working` (set `WORKING_DEMO=1` for the demo) and build the installer with `pwsh scripts/package-windows.ps1`, which needs `cargo install cargo-packager --version 0.11.8 --locked`.
