# RCWLP

Use an Android phone with a clip-on controller (Backbone, Kishi, GameSir, an Xbox or
DualSense pad over Bluetooth, ...) as a regular Xbox 360 controller on your Windows PC.

Unlike Moonlight, nothing is streamed: the PC doesn't capture or encode anything. Only
controller input travels over the network, in packets of 80 bytes, and rumble comes
back the other way. The phone screen stays black and dims itself to save battery.

```
 Phone + controller ──UDP──► rcwlp-host ──ViGEmBus──► virtual Xbox 360 pad ──► games
        ▲                         │
        └──────── rumble ─────────┘
```

## Parts

| folder | what | language |
|---|---|---|
| `host/` | PC app. Pairs phones and creates a virtual controller per phone. | Rust |
| `android/` | Phone app. Finds the PC and forwards the attached controller. | Kotlin |
| `PROTOCOL.md` | The wire format both sides speak. | |

## PC setup

1. Install the [ViGEmBus driver](https://github.com/nefarius/ViGEmBus/releases), if it
   isn't installed already. Sunshine installs it too.
2. Run the installer, `RCWLP-host-setup-<version>.exe`. It installs for your Windows user
   only, needs no admin rights, and starts the host in the notification area, now and
   whenever you sign in (untick that in the installer, or later in the tray menu). If the
   driver is missing, it offers to open the driver's download page. To build the installer
   yourself, install Rust and [Inno Setup 6](https://jrsoftware.org/isinfo.php)
   (`winget install JRSoftware.InnoSetup`), then:
   ```bash
   powershell -ExecutionPolicy Bypass -File installer/build.ps1
   ```
   It lands in `installer/output/`. Uninstalling (Settings → Apps) asks whether to forget
   the paired phones too; by default they're kept for a later reinstall.
3. The first time, Windows Firewall asks about network access. Allow it on **private**
   networks. Without that, the phone can't find the PC.

The tray app sits in the notification area: when a phone asks to pair, a small window asks
for the code; left-click the icon for the log (where the console commands below also work);
right-click for *Start with Windows* and *Quit*. Only one copy runs at a time.

The same host also runs in a console window, `rcwlp-host.exe` in the install folder
(`%LOCALAPPDATA%\Programs\RCWLP`). Quit the tray first, since both use the same port. To run
either straight from the source instead:
```bash
cd host
cargo run --release --bin rcwlp-tray
```
```bash
cargo run --release --bin rcwlp-host
```

Host commands, typed in the console window (or the tray app's log window):

| command | does |
|---|---|
| the phone's 6-digit code | accept a pairing request (`123456` or `123 456`) |
| `n` | reject a pairing request |
| `list` | show paired phones and who is connected |
| `forget <n>` | unpair phone number `n` |
| `quit` | unplug all virtual controllers and exit |

Pairings are stored in `%APPDATA%\RCWLP\host.json`. Use `--port <n>` to change the UDP
port from the default 47391. The phone's "Add PC by IP" then needs `ip:port`. If a phone
can't connect, run with `--verbose`: the host then logs every packet it ignores and why
(wrong version, not paired, bad signature, ...).

## Phone setup

Install `RCWLP-<version>.apk` on the phone: open it from your browser's downloads or a file
manager, and allow installing from that app when Android asks. It needs Android 8.0 or
newer.

To build it yourself, use Android Studio (open the `android/` folder) or a terminal:

```bash
cd android
./gradlew assembleDebug
```

Then install `android/app/build/outputs/apk/debug/app-debug.apk`, for example with
`adb install`. A build you make and the published APK are signed with different keys, so
to switch from one to the other, uninstall the app first (and pair again).

## Using it

1. Start the host on the PC, if its icon isn't in the notification area already.
2. Open RCWLP on the phone. The PC appears in the list (or use **Add PC by IP**).
3. Tap it. The phone shows a 6-digit code; type it in the PC window. If you cancel and
   tap again, the phone shows a new code, and the PC takes that one instead.
4. The controller screen opens and the PC gets a new Xbox 360 controller. Play.

Next time, just tap the paired PC. Each phone becomes its own player, so several phones
work at once. A phone with several controllers attached (say a clip-on and a Bluetooth
pad) gives the PC one controller for each, up to four, and each keeps its number while the
others come and go.

While connected, the controller screen shows the round trip to the PC in milliseconds, and
the host's `list` shows it too, with the phone's battery. Forgetting a PC on the phone also
unpairs the phone on the PC when the PC is reachable.

## Background mode (screen off, other apps)

Android normally only gives controller input to the app on screen. With **Keep working
when locked or in other apps** ticked, RCWLP reads the controller directly from the
kernel (`/dev/input`) instead, through [Shizuku](https://shizuku.rikka.app/), and
*grabs* it. The phone then ignores the controller entirely (B doesn't go Back, presses
don't wake the screen), and you can lock the phone or use other apps while it drives the
PC. A notification shows the connection and has a Disconnect button. Only controllers on
USB or Bluetooth are grabbed; built-in buttons, like a gaming phone's shoulder triggers, keep
working normally.

Setup, once per phone:
1. Install Shizuku and start it (Developer options → Wireless debugging, then "Start" in
   Shizuku). On phones without root this is needed again after every reboot. Over adb:
   `adb shell $(dirname $(pm path moe.shizuku.privileged.api | cut -d: -f2))/lib/arm64/libshizuku.so`.
2. In RCWLP, tick the checkbox and tap **Allow in Shizuku**.

How it works: `ControllerService` (a foreground service with wake and Wi-Fi locks) asks
Shizuku to start `EvdevService` in a separate process with adb privileges. That process
uses a small C module (`android/app/src/main/cpp/evdev.c`) to open and grab controllers.
It maps them with the same key-layout (`.kl`) files Android uses, so buttons match
foreground mode. If the app stops, the process exits and the phone gets the controller back.

**Screen off, and phones that freeze apps.** Some phones' power managers freeze apps
while the screen is off, foreground service or not. Honor's, for example, freezes RCWLP for
a quarter second every three seconds, which shows up as hiccups and missed or stuck
buttons. Two things help:

- In Xbox mode the reader process sends the input to the PC itself. It runs as the shell
  user, which those power managers leave alone, so input keeps flowing even while the app
  is frozen; the app only connects and handles rumble. It also repeats each change and
  holds very short taps for 20 ms, so a lost Wi-Fi packet or a game that checks the
  controller once a frame doesn't miss them. (PS4 mode still sends from the app, which
  reads the motion sensors.)
- Exempt RCWLP from the phone's power management. On Honor and Huawei: Settings →
  Battery → App launch → RCWLP → Manage manually, with all three switches on. Other
  makers have similar settings (Xiaomi: battery saver "No restrictions"; Samsung: "Never
  sleeping apps").

## Notes and limits

- **Without background mode the app must stay on screen.** The screen dims to almost
  black after 10 seconds; tap it to wake it.
- **Security.** Pairing uses P-256 ECDH and a code derived from the key, which you type
  on the PC. The phone commits to its key before it sees the PC's, so someone in the
  middle can't search for keys that make the codes match. Every packet after that is
  signed with HMAC-SHA256 and replay-protected. Input isn't encrypted, since button
  presses aren't secret, but nobody else on the network can inject input.
- **Dead zones.** If a worn stick drifts or a trigger never quite releases, raise *Stick dead
  zone* or *Trigger dead zone* on the main screen. Movement inside the zone reads as rest,
  and the rest of the travel is stretched so full deflection still reaches the maximum.
  Changes apply from the next connection.
- **Rumble** plays on the controller if Android exposes its motors (most Bluetooth pads
  on Android 12+), otherwise on the phone.
- **PS4 mode (gyro aiming).** Tick *PS4 controller mode* in the app and the PC gets a
  DualShock 4 instead of an Xbox pad, fed by the phone's gyroscope and accelerometer.
  Steam Input can then do gyro aiming: in a game's controller settings, set gyro to
  "As Mouse" or "As Joystick". Few games read DS4 motion on their own, so Steam is the
  usual way. In PS4 mode the phone screen is the DS4 touchpad: slide a finger to touch,
  tap to click. The phone's battery shows as the controller's, and when a game colours the
  DS4 lightbar, the controller screen takes on that colour.
- **ViGEmBus** is no longer developed upstream, but it still works and is what Sunshine
  uses. `host/vendor/vigem-client` is a copy of the `vigem-client` crate (MIT), extended
  with DS4 motion reports and DS4 rumble, which the published crate lacks.

## Development

- `cd host && cargo test` runs the host's tests: pairing, sessions and timeouts against a fake
  network and clock (`host/src/server/tests.rs`), plus the shared protocol test vectors.
- `cd android && ./gradlew testDebugUnitTest` runs the app's unit tests, including the same
  vectors (`protocol-vectors.txt`). If both pass, the PC and the phone agree on every byte.
- `cargo run --example fake_phone` plays the phone against a running host;
  `--discover` lists the hosts on the network and the protocol version each speaks.
- `cargo run --example xinput_watch` prints what games read from the virtual Xbox pads.
- `cargo run --example ds4_watch` reads the virtual DualShock 4 the way a game does and prints
  its buttons, sticks, touchpad and battery (`--motion` adds gyro and accelerometer); `--feedback R G B LARGE SMALL` sends a lightbar colour and
  rumble like a game, to check they reach the phone.
- The host's icon is drawn by `host/assets/make-icons.ps1`, which writes `tray.png` and
  `rcwlp.ico`; `host/build.rs` embeds the `.ico` and version info in the programs. The
  installer script is `installer/rcwlp-host.iss`.
- Turn on the pre-commit hook once per clone with `git config core.hooksPath .githooks`. It
  runs whichever of the two test suites the staged files affect.

### Making a release

`release.ps1` builds the PC installer and the signed phone app and puts them in `dist/`
with their SHA-256 checksums, ready to attach to a GitHub release:

```bash
powershell -ExecutionPolicy Bypass -File release.ps1
```

It needs Rust, Inno Setup 6, JDK 17 or newer in `JAVA_HOME`, and the release signing key.
The key isn't in the repository: `android/keystore.properties` (git-ignored) names the
keystore file and its passwords:

```
storeFile=C:/Users/you/.android/rcwlp-release.jks
storePassword=...
keyAlias=rcwlp
keyPassword=...
```

Keep a backup of both. Android only installs an update over an app signed with the same
key, so without it, people would have to uninstall and pair again to update. Raise
`versionCode` and `versionName` in `android/app/build.gradle.kts` and `version` in
`host/Cargo.toml` for each release.

## License

Source available under the Apache License 2.0 with the **Commons Clause** (see
[LICENSE](LICENSE)). In plain words: use it, change it and share it freely, as long as the
license goes with it; but don't sell it, or sell a product, hosting, setup or support whose
value is mainly RCWLP itself. `host/vendor/vigem-client` keeps its own MIT license.
