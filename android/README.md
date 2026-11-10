# Android screen viewer PoC

The Android host is the Tauri project in `src-tauri/gen/android/`. It builds the
same `src-ui` pages and Rust core as the desktop app. `android/` contains only
Android transport, service discovery, video decoding and their build dependencies.

The client connects **directly to the other Android device**, without a PC, adb
server, root, screen-capture companion app or relay. Both devices need Android 11+
for this prototype. The target must enable Wireless debugging and explicitly pair
the viewer. Wi-Fi isolation, VPNs and firewalls may prevent direct connections.

## What it implements

- The shared device list, pairing dialog, settings, language packs and themes.
- Android NSD discovers the same wireless-debugging services as desktop mDNS.
  The shared Rust monitor owns known-device matching, reconnects and retry policy.
- `AndroidAdb` implements the existing `AdbApi` using the native TLS transport.
- Android's file share sheet has four fixed entries: send/open, each to a chosen
  or default device. URI grants and private staging are Android-specific; device
  selection, default-device fallback, file transfer/opening and the request page
  use the same services and UI as desktop. Imports are released after cancellation
  or completion, even if their progress page is closed first. Single and multiple
  file shares are accepted; text-only shares are not converted into files.
- `AppService` and `SessionService` own app queries, launch settings and session
  state on every platform. They pass typed requests to `ScrcpyHost`: the desktop
  host runs the official client, while the Android host uses native transport
  and MediaCodec. Neither shared service calls Android RPCs or builds native UI.
- Pairing code + pairing port, followed by automatic discovery of the connection port.
- ADB identity generated in the app's private, non-backup storage; the code is not saved.
- ADB `sync:` upload and shell launch of the official scrcpy-server.
- ADB `localabstract:` video transport, H.264 hardware decoding through MediaCodec,
  aspect-ratio preservation and decoder restart on a remote orientation change.
- The scrcpy control socket forwards taps, long presses, swipes and multiple touch pointers,
  mapping the displayed surface coordinates to the encoded video dimensions.
- The remote server supplies the app list; parsing, caching, favourites, recent
  apps and launch policy remain shared. App launch uses the shared protocol
  encoder and supports the existing main-display/virtual-display setting.
- Each screen or app session has its own resizable Android task and control
  channel. Public launch bounds request a smaller window; freeform window
  availability is decided by Android and the device manufacturer. Unsupported
  devices may show a full-screen task instead. Closing its task stops the session.
- Touch coordinates come from the native surface; control packet encoding lives
  in the same Rust protocol adapter used by desktop sessions.
- Version selection uses the shared release repository, checksum verification,
  download progress and saved choice. Android installs the server asset instead
  of a desktop client archive. Only versions supported by the native presenter
  are offered (currently 5.0.1); the bundled server seeds that same repository.
- Desktop process/window settings, add-on folder installation and desktop file
  drag-and-drop are hidden. Audio, keyboard input and background services are not
  implemented by the Android viewer. Sessions can survive a temporarily detached
  video surface, but there is no foreground service to prevent Android from
  suspending or terminating the process in the background.

The scrcpy server version comes from `src-tauri/src/core/scrcpy/versions.rs`, and
the app version comes from `src-tauri/Cargo.toml`. The build downloads the server
from its official GitHub release and verifies SHA-256 against `SHA256SUMS.txt`.
The video parser follows the existing desktop `src-tauri/src/core/scrcpy/v5.rs`
session/config/key-frame protocol, not the older v4 documentation.

## Build

The normal `npm run package` also exports the Android APK, alongside the
desktop packages and add-on types. `--sequential` applies to all three builds.
To build only the APK, without rebuilding the desktop apps:

```sh
docker build --target export-android --output type=local,dest=out.tmp-android .
```

The Docker build currently targets ARM64. For a local build, install the Tauri
Android prerequisites and NDK, then run `npm run tauri -- android build --target aarch64 --apk`.
Do not commit `local.properties` or authentication keys. Docker supplies the SDK
without installing Node, Gradle or Android tools on the host.

## Signing and device validation

`npm run package` uses these optional environment variables. For GitHub CD, add
them under **Settings → Secrets and variables → Actions → Repository secrets**:

| Variable / GitHub secret | Value |
| --- | --- |
| `ANDROID_KEYSTORE_BASE64` | Base64-encoded contents of the signing `.jks` or `.keystore` file, not its path |
| `ANDROID_KEYSTORE_PASSWORD` | Keystore password |
| `ANDROID_KEY_ALIAS` | Alias of the signing key in that keystore |
| `ANDROID_KEY_PASSWORD` | Password of that key; provide this even when it matches the keystore password |

With all four supplied, packaging aligns, signs and verifies the APK, then exports
`scrcpy-agent_<version>_android.apk`. Without any signing variables it exports
`scrcpy-agent_<version>_android_unsigned.apk`. Partial configuration fails before
building; signing or verification failure stops packaging. Release checksums are
calculated after signing, and the signed APK goes into the existing draft release.

The key and passwords use Docker secret mounts; the decoded keystore stays in a
temporary memory-backed mount. The signing stage always runs with the current
secrets, while the unsigned APK compilation can reuse its build cache. Keep the
same signing key for subsequent updates and retain a backup outside the repository.

The direct `export-android` Docker target above remains unsigned. Gradle does not
generate a debug keystore or release signing key. Android will not install an
unsigned APK, including through `adb install`. An ADB authentication certificate
is a separate protocol credential generated at app runtime, not an APK signature.

After installing an owner-signed copy:

1. On the target, enable Developer options → Wireless debugging.
2. Use the shared Pair device dialog. Select the discovered pairing address or
   enter the target's IP and pairing port, then enter its six-digit code.
3. After pairing the shared monitor connects the target using its mDNS connection
   service. Previously known devices reconnect automatically while the app runs.
4. Open its screen from the device card. Rotate the target to check resizing;
   close the screen's task to stop. Network isolation and background
   throttling still apply; there is no Android foreground service.

Compilation is not proof of successful pairing or video playback. Earlier PoC
versions were exercised on devices. The shared-host refactor has automated
host-level coverage; independent session windows still need on-device validation.

## Protocol dependencies

- [libadb-android 3.1.1](https://github.com/MuntashirAkon/libadb-android/tree/3.1.1),
  used under its Apache-2.0 option; its transitive dependencies have separate license terms.
- [Conscrypt](https://github.com/google/conscrypt), for remote Android TLS connections.
- [Bouncy Castle](https://www.bouncycastle.org/), for the private ADB identity certificate.
- [Official scrcpy server](https://github.com/Genymobile/scrcpy), Apache-2.0.

Third-party licenses remain applicable to redistributed libraries. The protocol
library is linked as a dependency, rather than copied into this repository.
