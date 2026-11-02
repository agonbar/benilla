# benilla beta 1: Steam Deck pad and Quest 3 VR

This is the guide for the `v0.3.0-beta1` build. One binary does both jobs. Built with `--features vr` it plays flat with the Deck's pad, and in the headset when an OpenXR runtime is running. Read it top to bottom the first time.

Every binding and setting below comes from the code on `main` at `b1990211` (parts 1 and 2 of the beta); the "Source" column names the file. If the game disagrees with this file, the game is right: report it (last section).

What is not here: WoW 1.12.1 client data. It never goes in this repo. You supply it on the Deck (section 2).

## 1. Build for the Deck

The Deck's distrobox is Fedora 42 (glibc 2.41). A build made in a plain `nix-shell` on work-vm-00 links newer glibc symbols and does not start there. Build in a `fedora:42` container instead.

The build script is `~/Documents/projects/vmangos/scripts/build-benilla-deck.sh` on work-vm-00. It is not part of this repo. It takes the path of a benilla checkout and does this:

1. Starts a `fedora:42` podman container with the checkout at `/src`.
2. Installs the build dependencies (`gcc`, `pkgconf-pkg-config`, `alsa-lib-devel`, `systemd-devel`, `wayland-devel`, `libxkbcommon-devel`, `rustup`).
3. Installs the toolchain `rust-toolchain.toml` pins, into a cache directory the script names in `T=` at its top. The second build reuses it, so only changed crates rebuild.
4. Runs `cargo build --release -p benilla --features vr`.

`--features vr` implies `pad` (`crates/benilla/Cargo.toml`). That one binary is the pad client, the VR client, and a flat pad client when no OpenXR runtime is up (`crates/benilla/src/main.rs`). Do not use the `sed` that swaps in `--features pad`, which the ADR-29 notes used for the flat-only test.

The output is `fedora42/release/benilla` under the cache directory `T`.

```bash
# On work-vm-00. <checkout> is a benilla clone at the beta commit.
git -C <checkout> fetch origin
git -C <checkout> checkout v0.3.0-beta1       # or the merge commit, if Lead has not tagged yet
cd ~/Documents/projects/vmangos
scripts/build-benilla-deck.sh <checkout>
```

Memory: a cold release build of benilla peaked at 9 GiB with 4 jobs, and the script sets `CARGO_BUILD_JOBS=4`. work-vm-00 has about 10 GiB shared with the desktop. I did not run this build for the beta PR. Close the heavy desktop apps first, or run it with two jobs without editing the script:

```bash
sed 's/CARGO_BUILD_JOBS=4/CARGO_BUILD_JOBS=2/' scripts/build-benilla-deck.sh | bash -s -- <checkout>
```

Copy the binary to the Deck:

```bash
scp -i ~/.ssh/nas "$T/fedora42/release/benilla" deck@<deck-address>:benilla/target/release/benilla
```

`$T` is the `T=` value from the script. `<deck-address>` is the Deck's address on your LAN. The `nas` key is the one the vault note `projects/benilla-vr.md` names for the Deck; do not use `id_ed25519`. The launcher below runs from `~/benilla` on the Deck, so that folder has to exist.

The build id is stamped at compile time. The first log line of every run reads `benilla build <release> · <short sha> · <commit date> · <profile>` followed by `· extended` (a crate on top of benilla added plugins). Compare the short sha with `git rev-parse --short HEAD` on the build commit. The release number will read `0.2.0`: the workspace version is still 0.2.0, and `BuildId::release` only appends `+N` for a tag of the same version (`crates/benilla-world/src/build_id.rs`). The sha is what identifies the beta.

## 2. Launch in Game Mode

### What the client needs

| Need | Value | Where it is read |
|---|---|---|
| State folder | `BENILLA_HOME=$HOME/benilla/benilla-config` | `crates/benilla-app/src/local_state.rs` |
| WoW 1.12.1 `Data` folder | `WOW_DATA=$HOME/wow/1.12.1/Data` | `crates/benilla-formats/src/install.rs` |
| Realm | `realmName = "VMaNGOS"` under `[cvars]` in `benilla-config/config.toml`; `WOW_REALM` overrides it | `crates/benilla-app/src/cvars/mod.rs`, `ui_script/glue_mgr.rs` |
| Server address | `WOW_HOST` | `crates/benilla-app/src/realmlist.rs` |
| Login | `WOW_USER` and `WOW_PASS` (both, or neither and type it with the on-screen keyboard) | `crates/benilla-app/src/run_mode.rs` |

`BENILLA_HOME` is not optional on the Deck. The binary is built on work-vm-00, and a build with the default `dev` feature (the script's) looks for `benilla-config/` at a path baked in at build time (`dev_project_root()`). On the Deck that path does not exist, so `config.toml` is never read and `realmName` starts empty. Pinning `BENILLA_HOME` skips that lookup. The real fix (a fallback when the baked folder is missing) touches upstream's `local_state.rs` and is not in this beta.

`WOW_DATA` points at the `Data` folder inside the 1.12.1 install, not at the install. The vault note puts the install at `~/wow/1.12.1`. Adjust the path if yours differs. Without an install the client prints `benilla: no WoW install found`, lists the folders it tried and exits. The data stays on the Deck.

Keep the account and the server address out of the repo and out of the launcher: `~/.config/benilla-vr/account.env` (mode 600) holds `WOW_USER` and `WOW_PASS`. This file is not in this repo.

### The launcher

Create `~/benilla/run-beta.sh` on the Deck and `chmod +x` it. `distrobox enter` drops host variables whose values hold spaces, so everything is set inside the box:

```bash
#!/usr/bin/env bash
# ~/benilla/run-beta.sh : benilla beta 1 in the Fedora box, log kept in ~/benilla/benilla.log
exec distrobox enter benilla -- bash -c '
  set -a; . "$HOME/.config/benilla-vr/account.env"; set +a
  export BENILLA_HOME="$HOME/benilla/benilla-config"
  export WOW_DATA="$HOME/wow/1.12.1/Data"
  export WOW_HOST="<server address of your realm>"
  # export WOW_PAD_TRACE=1     # one "pad: <token> down -> <COMMAND>" line per press
  cd "$HOME/benilla" && ./target/release/benilla 2>&1 | tee -a "$HOME/benilla/benilla.log"
'
```

Leave `WOW_REALM` out. If the realm picker shows up, set `WOW_REALM=VMaNGOS` temporarily and report it.

`~/benilla/benilla-config/config.toml` must hold:

```toml
[cvars]
realmName = "VMaNGOS"
```

### Check the config was read

Right after start:

```bash
grep -m1 'config:' ~/benilla/benilla.log
```

| Line | Meaning |
|---|---|
| `config: read N entries from config.toml` | Good. `realmName` reached the connect step. |
| `config: no config.toml at "<path>", running on defaults` | `BENILLA_HOME` is wrong or the file is not there. The path in the line is where it looked. |

### Add it to Steam

1. Switch the Deck to Desktop Mode and open Steam.
2. Games, Add a Non-Steam Game to My Library, Browse, set the file filter to all files, pick `~/benilla/run-beta.sh`, Add Selected Programs.
3. Right-click the entry, Properties, rename it "benilla". Leave the launch options empty.
4. Controller settings for the entry: use the Gamepad template. A layout that maps the pad to keys and mouse gives the client keyboard events and no pad. Run once with `WOW_PAD_TRACE=1` and look for `pad:` lines to be sure.
5. Back to Game Mode, Library, Non-Steam, benilla, Play.

### If the window does not show in Game Mode

gamescope only shows a window flagged with a Steam app id. A client that Steam did not start itself, or one started inside distrobox (which drops Steam's environment), has no flag. The ADR-29 run showed it: the client ran, the screen stayed on Steam. This was verified for a client started by hand over SSH. I could not test the Steam shortcut route, so it may need this step.

While benilla runs, flag its window on the Deck's host (not in the box). Use a spare app id, then restore the old base layer afterwards:

```bash
# Window id: the line with "benilla" in the window tree of the game display (:1)
win=$(DISPLAY=:1 xwininfo -root -tree | awk '/"benilla"/ {print $1; exit}')
APP=7777777
DISPLAY=:1 xprop -id "$win" -f STEAM_GAME 32c -set STEAM_GAME $APP
# Base layer on the display Steam's UI uses (:0). 769 was the layer before; keep it to restore.
DISPLAY=:0 xprop -root -f GAMESCOPECTRL_BASELAYER_APPID 32c -set GAMESCOPECTRL_BASELAYER_APPID $APP,769
# Later, put Steam's UI back:
DISPLAY=:0 xprop -root -f GAMESCOPECTRL_BASELAYER_APPID 32c -set GAMESCOPECTRL_BASELAYER_APPID 769
```

If `xwininfo` finds nothing, run `DISPLAY=:1 xwininfo -root -tree` and read the window id off the benilla line. The window title is `benilla` (`crates/benilla-app/src/lib.rs`). Steam's virtual pad kept delivering input after this in the ADR-29 run. Keyboard and mouse input from `xdotool` never reaches the window under gamescope, so do not use it to test.

### Flat play with the headset build

With no OpenXR runtime the log says `vr: OpenXR is unavailable (no loader, runtime or headset), running flat` and the pad plays on the Deck's screen. If the client hangs at start with the WiVRn server running and no headset connected, stop the server (`systemctl --user stop wivrn-server`) and start again. I have not seen this; it is a precaution.

### Pad settings

`~/benilla/benilla-config/pad.toml` is optional. benilla reads it and never writes it. On the Deck's 1280x800 screen this makes the UI fit:

```toml
ui_scale = "deck"
```

The other keys are listed in the comment at the top of `crates/benilla-input/src/config.rs`.

## 3. Default pad layout

Names follow the Deck as Steam presents it (an Xbox 360 pad): Menu is the right of the two small buttons, View the left. The tokens in the table (`PAD1`, `PADLSTICK`...) are what the Key Bindings window shows, and any binding there can be rebound. Defaults: `crates/benilla-input/src/layers.rs`.

The pad plays in two modes. Gameplay mode is the one after you enter the world. Cursor mode shows a pointer for windows.

### Gameplay mode

| Input | Does | Source |
|---|---|---|
| Left stick | Walks in 8 directions | `layers.rs` |
| Right stick | Turns the camera (held mouselook) | `look.rs` |
| A | `INTERACTORJUMP`: talks to, loots or opens what the soft target has in front, jumps when there is nothing | `layers.rs`, `soft.rs` |
| B | `ACTIONBUTTON2` | `layers.rs` |
| X | `ACTIONBUTTON3` | `layers.rs` |
| Y | `ACTIONBUTTON4` | `layers.rs` |
| D-pad down, right, left, up | `ACTIONBUTTON5`, `6`, `7`, `8` | `layers.rs` |
| Left stick click | `ACTIONBUTTON1` | `layers.rs` |
| Right stick click | `TOGGLEAUTORUN` | `layers.rs` |
| LB | Locks the next friend (soft target) | `soft.rs` |
| RB | Locks the next enemy (soft target) | `soft.rs` |
| LT held | Layer 2, see below | `layers.rs` |
| RT held | Layer 3, see below | `layers.rs` |
| LT and RT held | Layer 4, see below | `layers.rs` |
| LT + RB | `INTERACTTARGET`: interact with the soft target, never jumps | `layers.rs` |
| LT + LB, then d-pad up, right, down, left | Targets party member 1, 2, 3, 4 | `soft.rs` |
| LT + right stick click, or a back paddle on `PADPADDLE1` | `RADIALRING`, held (see below) | `layers.rs`, `ring.rs` |
| Menu | `TOGGLEGAMEMENU` | `layers.rs` |
| View, tap | Switches to cursor mode and back | `hold.rs`, `mode.rs` |
| View, held 0.6 s | Shows or hides the on-screen keyboard | `hold.rs` |

The soft target is the unit, corpse or object the client picks for you in front of the camera: up to 41 yd for units, 10 yd for corpses and objects, within 40 degrees either side of the camera. `pad.toml` can change those numbers (`soft_range`, `soft_object_range`, `soft_arc`). LB and RB each lock one selection per press and cycle on the next press.

Bar layers (the four rows of the same eight buttons):

| Hold | A / B / X / Y | D-pad down / right / left / up |
|---|---|---|
| Nothing | interact-or-jump / `ACTIONBUTTON2` / `3` / `4` | `ACTIONBUTTON5` / `6` / `7` / `8` |
| LT | `ACTIONBUTTON9` / `10` / `11` / `12` | `MULTIACTIONBAR1BUTTON1` / `2` / `3` / `4` |
| RT | `MULTIACTIONBAR1BUTTON5` / `6` / `7` / `8` | `MULTIACTIONBAR1BUTTON9` / `10` / `11` / `12` |
| LT and RT | `MULTIACTIONBAR2BUTTON1` / `2` / `3` / `4` | `MULTIACTIONBAR2BUTTON5` / `6` / `7` / `8` |

Main bar slot 1 is on the left stick click because A is the interact. Turn on the Bottom Left and Bottom Right action bars in Interface Options to see layers 3 and 4.

The radial ring opens while you hold its binding. The right stick picks an entry by angle (tilt at least halfway), and letting go runs it. The eight entries, clockwise from the top: Character, Spellbook, Talents, Quests, Bags, Map, Social, Keyboard. Movement keeps working while it is open.

The on-screen keyboard is a window of the shipped `BenillaPad` addon, written into `benilla-config/AddOns/BenillaPad/` at start. It has lower, upper and symbol pages, plus Backspace and Enter. Show it with a long press of View, the Keyboard entry of the ring, or the Kbd button by the chat frame (shown when `presentation = "controller"`, which `auto` picks once the pad was the last input). Click a key with X or A; the character goes to the focused edit box, so open the chat line first (the Kbd button does both).

### Cursor mode

Cursor mode starts when a window opens (bag, character, game menu, a popup), when a ground spell asks for a target, or when you tap View. It ends when the windows close, when you tap View again, or when the left stick walks.

| Input | Does | Source |
|---|---|---|
| Right stick | Moves the cursor (1100 px/s at full travel, `cursor_speed`) | `config.rs` |
| A | Right click: use, sell, deposit | `actions.rs` |
| X | Left click: pick up, put down | `actions.rs` |
| Y | Shift-left click: split a stack, link | `actions.rs` |
| B | Closes the top window, or cancels a targeting cursor | `actions.rs` |
| D-pad | Snaps the cursor to the next widget up, down, left, right | `actions.rs` |
| LB, RB | Scroll wheel up, down | `actions.rs` |
| Left stick | Walks, which leaves cursor mode | `mode.rs` |
| View | Back to gameplay mode (tap) or the keyboard (hold) | `hold.rs` |

## 4. VR on the Quest 3

The route is WiVRn: the Deck runs the OpenXR runtime (WiVRn server) and streams to the WiVRn app on the headset. benilla runs in the Fedora distrobox and finds the runtime through the OpenXR loader (`libopenxr_loader.so`, from the box's `openxr-devel`).

1. On the Deck, start the server as a user unit (this is the form the vault note records; `--no-encrypt` means no pairing PIN, so use it on a trusted LAN only):

   ```bash
   systemd-run --user --unit=wivrn-server flatpak run --command=wivrn-server io.github.wivrn.wivrn --no-encrypt
   ```

2. In the WiVRn app on the Quest, add the Deck by hand (its address, port 9757). avahi discovery is off. Connect.
3. Start benilla on the Deck with the launcher of section 2. That launcher does not point the OpenXR loader at WiVRn. `~/benilla/run-vr.sh` on the Deck does that setup and is not in this repo, so I could not check it: copy its environment lines into `run-beta.sh`. The lines to look for in the log:
   - `vr: comfort person first turn snap ...` means the VR plugin started and read `vr.toml`.
   - `vr: OpenXR is unavailable (no loader, runtime or headset), running flat` means the loader or the runtime is missing. OpenXR's own error just above it says which.

The client window on the Deck stays a flat mirror of the UI, and the world shows only in the headset. Check input in the log (`WOW_VR_INPUT_TRACE=1`), not with a screenshot of the Deck. The Deck's GPU runs near 100 percent at WiVRn's default per-eye resolution, so expect judder and black edges on fast head turns (vault note, 2026-10-20). Lowering the resolution in the WiVRn app is the first thing to try.

### Touch controllers

The Touch controllers are a gamepad for the same input layer the Deck uses (`crates/benilla-vr/src/touch.rs`), so every binding in section 3 works in the headset with this map:

| Touch | Pad | Does in gameplay mode |
|---|---|---|
| A | A | Interact or jump |
| B | B | `ACTIONBUTTON2` |
| X | X | `ACTIONBUTTON3` |
| Y | Y | `ACTIONBUTTON4` |
| Left grip | LB | Locks the next friend |
| Right grip | RB | Locks the next enemy |
| Left trigger | LT | Layer 2 when held; with the laser pointing, a tap interacts with what it hovers |
| Right trigger | RT | Layer 3 when held; with the laser pointing, a tap locks what it hovers |
| Both triggers | LT and RT | Layer 4 |
| Left thumbstick | Left stick | Walks |
| Right thumbstick | Right stick | Turns (below), no vertical action |
| Left thumbstick click | Left stick click | `ACTIONBUTTON1` |
| Right thumbstick click | Right stick click | `TOGGLEAUTORUN` |
| Left menu button | View | Tap: cursor mode. Hold 0.6 s: keyboard |
| Right menu button | none | The Quest's system button, the game never sees it |

There is no d-pad on Touch, so `ACTIONBUTTON5` to `8` and the d-pad slots of layers 2 to 4 have no button in the headset. The game menu (Menu, `PADFORWARD`) has none either. Put the spells you need on A, B, X, Y and the left stick click, or rebind. Open the game menu with the laser on the micro-menu bar, or with Escape on a keyboard. LT + right stick click opens the radial ring as on the Deck.

### The UI laser

The right controller carries the laser. It runs from the controller's aim pose onto the UI panel, the floating screen in front of you.

- On the panel the pad goes to cursor mode, with the laser as the pointer. The right trigger is a left click, and held it drags. The right thumbstick scrolls, 12 notches a second at full travel.
- A, X, Y, B, and the grips act as in cursor mode (section 3): A right-clicks, X left-clicks, Y shift-clicks, B closes the window, the grips scroll.
- Walking with the left stick while the laser is on the panel does not leave cursor mode.

### Pointing at the world

Off the panel, in gameplay mode, the laser hovers units, corpses and objects. The beam ends at what it hits, and the end marker is bright on something you can act on. The right controller gives a short haptic tick each time the hover moves to a new unit or object.

- Tap the right trigger: lock (target) the hovered unit.
- Tap the left trigger: interact with it (talk, loot, open).
- A ground spell (Blizzard, for example) puts the pad in cursor mode; the right trigger then places it where the laser points.

A tap is a press and release with no other button between. A held trigger is a layer, not a tap, so using the bar under a trigger never retargets.

### Comfort settings

Comfort lives in `vr.toml` in the state folder (`$BENILLA_HOME/vr.toml`). benilla writes it with the defaults on first run, and re-reads it about once a second while a session runs, so you can change it without restarting. The log says `vr comfort: reloaded vr.toml: ...` (with `WOW_VR_INPUT_TRACE=1`) and any bad value as a `vr: vr.toml: ...` warning. Source: `crates/benilla-vr/src/comfort.rs`.

| Setting | Key in `vr.toml` | Default | Values |
|---|---|---|---|
| First or third person | `person` | `"first"` | `"first"` (the camera is your head), `"third"` (behind the avatar) |
| Distance in third person | `third_person_distance` | `5.55` | 1 to 15 yards |
| Right stick turn | `turn` | `"snap"` | `"snap"` (a flick turns by `snap_degrees`), `"smooth"` (held turns) |
| Snap step | `snap_degrees` | `30` | 5 to 180 degrees |
| Smooth rate | `smooth_degrees_per_second` | `120` | 15 to 360 |
| Vignette on or off | `vignette` | `true` | `true`, `false` |
| Vignette strength | `vignette_strength` | `0.5` | 0 to 1 |

A snap fires when the stick passes 70 percent of its travel and re-arms below 30 percent. The vignette closes in while the camera moves or smooth-turns, over 0.2 s, and opens over 0.4 s. A file with a key benilla does not know is read as all defaults, with one warning.

Recentre is not a setting. Hold both thumbstick clicks for one second and the view moves to put your head at the game camera's position, facing forward, whether you sit or stand. The right click does not toggle autorun while the left one is down.

## 5. Known limitations

In this beta:

- The overlay chain is out: chat bubbles as world cards ([ADR-36](/ADR/issues/ADR-36)), combat text over units ([ADR-37](/ADR/issues/ADR-37)) and V-plates ([ADR-38](/ADR/issues/ADR-38)). Those tickets are blocked. They are not placed in the world in the headset. Do not file them as bugs.
- Seven of the Deck and headset check lists are still open: [ADR-29](/ADR/issues/ADR-29) to [ADR-35](/ADR/issues/ADR-35) are all `todo`. The code has unit tests; the hardware has barely been touched. Expect real bugs, especially in ADR-30 to ADR-35: the pad cursor, soft target and cycling, the radial ring and keyboard, the Touch controllers and laser, world pointing, and VR comfort.
- ADR-29 got a partial run (Game Mode, `6b9e42c9`). It passed: the left stick in every direction, the right stick as look, A, B, X, Y and the d-pad on their bar slots, RB targeting the nearest enemy, and Menu opening the game menu. It did not exercise LB, the stick clicks, a bar press under a trigger, or the Key Bindings rows.
- Part 1 ([ADR-46](/ADR/issues/ADR-46)) fixed a pad that vanishes mid-session by making gilrs watch `/dev/input` instead of udev in a container. It is not yet seen on a Deck. In that mode gilrs may list the raw `Steam Deck` node next to Steam's virtual pad. `grep 'Gamepad .* connected' ~/benilla/benilla.log` should show one pad per connection.
- The `realmName` empty at connect is a deploy issue (section 2), not a code regression. The proper fix (the baked state folder falling back when it is missing) is not in this beta. The new `config:` line is the check.
- There is no rumble: nothing in benilla sends it.
- In the headset there is no d-pad and no Menu button (section 4). Hands tracking is off. The reference space is seated (`LOCAL`).
- The Deck's GPU is the bottleneck in VR (section 4).
- Part 2 ([ADR-47](/ADR/issues/ADR-47)) ran the tests green, with these gaps: the `reference` and `enforcer` gates need a `benilla-re` checkout and a WoW install; the `enforcer-no-install` gate fails without a display in `gates.sh` (it passes under Xvfb); `scripts/smoke.sh` needs a server and a window. Building with the nix-shell recipe takes Rust 1.99, which fails clippy on five upstream lines, where `rust-toolchain.toml` pins 1.98.1. Two tickets for those were proposed and not opened.

## 6. Reporting a bug

Say which step of this file you were on, what you did and what you expected. Then attach:

1. The build line and the first `config:` line:

   ```bash
   grep -m1 'benilla build' ~/benilla/benilla.log
   grep -m1 'config:' ~/benilla/benilla.log
   ```

   The same build id is in the footer of the debug panel in the game.
2. The log, `~/benilla/benilla.log` (written by the launcher of section 2), from the start of the run. Check it for your account name and server address before pasting; never paste `WOW_PASS`.
3. After a crash, the newest `~/benilla/benilla-config/Diagnostics/crash-<unix-seconds>.txt`.
4. For input bugs, a run with `WOW_PAD_TRACE=1` (the `pad:` lines). In the headset also `WOW_VR_INPUT_TRACE=1` (one line per Touch change, snap turn, recentre and setting change).
5. For a pad that goes dead, the answer to `ls /run/.containerenv /.dockerenv; echo $CONTAINER_ID` run inside the box.
6. For VR, the `vr:` lines and the `vr comfort:` line, and the contents of `vr.toml`.
