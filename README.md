# RCWLP

Use an Android phone with a clip-on controller (Backbone, Kishi, GameSir, an Xbox or
DualSense pad over Bluetooth, ...) as a regular Xbox 360 controller on your Windows PC.

Unlike Moonlight, nothing is streamed: the PC doesn't capture or encode anything. Only
controller input travels over the network, in packets of 66 bytes, and rumble comes
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
2. Build and run the host:
   ```bash
   cd host
   cargo run --release
   ```
3. The first time, Windows Firewall asks about network access. Allow it on **private**
   networks. Without that, the phone can't find the PC.

Host commands, typed in its window:

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

Build the app with Android Studio (open the `android/` folder), or from a terminal:

```bash
cd android
./gradlew assembleDebug
```

Then install `android/app/build/outputs/apk/debug/app-debug.apk`, for example with
`adb install`. It needs Android 8.0 or newer.

## Using it

1. Run `rcwlp-host` on the PC.
2. Open RCWLP on the phone. The PC appears in the list (or use **Add PC by IP**).
3. Tap it. The phone shows a 6-digit code; type it in the PC window. If you cancel and
   tap again, the phone shows a new code, and the PC takes that one instead.
4. The controller screen opens and the PC gets a new Xbox 360 controller. Play.

Next time, just tap the paired PC. Each phone becomes its own player, so several phones
work at once.

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

## Notes and limits

- **Without background mode the app must stay on screen.** The screen dims to almost
  black after 10 seconds; tap it to wake it.
- **Security.** Pairing uses P-256 ECDH and a code derived from the key, which you type
  on the PC. The phone commits to its key before it sees the PC's, so someone in the
  middle can't search for keys that make the codes match. Every packet after that is
  signed with HMAC-SHA256 and replay-protected. Input isn't encrypted, since button
  presses aren't secret, but nobody else on the network can inject input.
- **Rumble** plays on the controller if Android exposes its motors (most Bluetooth pads
  on Android 12+), otherwise on the phone.
- **PS4 mode (gyro aiming).** Tick *PS4 controller mode* in the app and the PC gets a
  DualShock 4 instead of an Xbox pad, fed by the phone's gyroscope and accelerometer.
  Steam Input can then do gyro aiming: in a game's controller settings, set gyro to
  "As Mouse" or "As Joystick". Few games read DS4 motion on their own, so Steam is the
  usual way. The touchpad isn't sent yet.
- **ViGEmBus** is no longer developed upstream, but it still works and is what Sunshine
  uses. `host/vendor/vigem-client` is a copy of the `vigem-client` crate (MIT), extended
  with DS4 motion reports and DS4 rumble, which the published crate lacks.
