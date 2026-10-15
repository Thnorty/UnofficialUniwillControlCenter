# Linux hardware test

The checks to run on the laptop before a Linux release, from a Fedora live USB (KDE Plasma, Secure Boot off), where
everything installed is gone after a reboot. Tick each line; for a failure, note what you saw and fix it with a test
first.

## Setup

1. Build both packages in WSL (`wsl -e bash scripts/build-linux.sh`, then `wsl -e bash scripts/build-linux.sh
   --check`) and copy `dist/unofficial-uniwill-control-center-<version>-1.x86_64.rpm` to the Ventoy stick.
2. Boot the Fedora live USB and check Secure Boot is off: `mokutil --sb-state` says `SecureBoot disabled`.
3. Install: `sudo dnf install ./unofficial-uniwill-control-center-<version>-1.x86_64.rpm`.
4. To read an EC register without the app (as the probe does), keep this at hand; it prints the byte at `0x7C6`:

   ```sh
   sudo python3 -c "import mmap,os; f=os.open('/dev/mem', os.O_RDONLY|os.O_SYNC); m=mmap.mmap(f, 4096, mmap.MAP_SHARED, mmap.PROT_READ, offset=0xFE410000); print(hex(m[0x7C6]))"
   ```

## Install

| | Check | Do | Expect |
|---|---|---|---|
| ☐ | Files | `rpm -ql unofficial-uniwill-control-center` | The app, `uucc`, `/usr/libexec/uucc-helper`, both units, the polkit action, the udev rule, the desktop entry and the icon. |
| ☐ | Helper socket | `systemctl status uucc-helper.socket` | `active (listening)`; `uucc-helper.service` inactive until the app connects. |
| ☐ | Lighting access | `getfacl /dev/hidraw*` for the keyboard and light bar | Your user has `rw`. If not, sign out and back in, and check again. |
| ☐ | App menu | Open **Unofficial Uniwill Control Center** from the app menu | Its icon is there; the window opens. |

## First start and the window

| | Check | Do | Expect |
|---|---|---|---|
| ☐ | Risk question | First start | A kdialog warning with OK/Cancel that mentions the helper running as root; OK starts the app. |
| ☐ | Hidden pages | Look at the navigation | No Display or Plugins page; no Typing, Music or Spectrum tiles; Settings has no idle timer, touchpad or Windows key rows and says "Start at login". |
| ☐ | Icons and font | Every page | Every icon draws (no empty boxes); text in the desktop's font. |
| ☐ | Theme | Switch Plasma between Breeze Light and Breeze Dark | The window follows. |
| ☐ | Helper started | `systemctl status uucc-helper.service` | `active (running)` while the app runs. |

## Performance and fans

| | Check | Do | Expect |
|---|---|---|---|
| ☐ | Modes | Office, Gaming, Turbo in turn | The Performance page and Monitor follow; the fans' sound and RPM change as on Windows; the power limits shown follow the mode. |
| ☐ | Fan Boost | On, then off | The fans go to full speed and back. |
| ☐ | Profiles | Add a profile with custom PL1/PL2, apply it, switch back | The Monitor's CPU power follows the limits under load. |
| ☐ | Fan curve | Turn a custom curve on, change a point, turn it off | The fans follow it on; on the laptop's curve again when off; `0x7C6` bit 2 (value `& 0x04`) set while on and clear after. |
| ☐ | Crash with a curve on | Curve on, then `pkill -9 -f unofficial-uniwill-control-center` | Within a second `0x7C6 & 0x04` is 0 and the fans follow the laptop's curve. |
| ☐ | Fan safety | (If it can be triggered) heavy load with a low custom curve | The app steps back to the laptop's curve and notifies. |

## Battery

| | Check | Do | Expect |
|---|---|---|---|
| ☐ | Battery page | Open it | Charge, state, capacities, rate and cycle count as the kernel reports them. |
| ☐ | Charge modes | Full, Balanced, Long life | The setting stays after reopening the app; the charge stops at the mode's level. |
| ☐ | USB charging while off | Turn it on, power off, plug a phone into the USB port | The phone charges. |

## Lighting

| | Check | Do | Expect |
|---|---|---|---|
| ☐ | Keyboard effects | Each firmware effect, brightness and speed | The keyboard follows as on Windows. |
| ☐ | Light bar | Each effect, on/off, "Same as keyboard" | The light bar follows. |
| ☐ | Smooth Wave and status effects | Wave on the keyboard; Battery, CPU load and the temperatures on the light bar | They animate and follow the readings. |
| ☐ | Setups | Apply a saved setup | Lighting and mode change together. |

## BIOS settings

| | Check | Do | Expect |
|---|---|---|---|
| ☐ | Back up first | `sudo cp /sys/firmware/efi/efivars/UniWillVariable-9f33f85c-13ca-4fd1-9c4a-96217722c593 ~/UniWillVariable.bak` and copy it to the stick | A 184-byte file. |
| ☐ | Read | Open the Performance page's GPU mode and voltage offset | The values the BIOS setup shows. |
| ☐ | Write | Change the boot lighting, then restore it | The app reports success; `lsattr` on the variable still shows `i` (immutable) afterwards. |

## Desktop

| | Check | Do | Expect |
|---|---|---|---|
| ☐ | Tray | The icon in the panel; left click; the menu | The icon shows the keyboard colors; a click opens the window; the menu's lighting, mode and timer items work and their checks are current. |
| ☐ | Notifications | Start a 1-minute timer | A desktop notification when it ends. |
| ☐ | Charger | Unplug and plug in the charger | The power limits and the Battery page follow within a couple of seconds. |
| ☐ | Sleep | Suspend and wake | Lighting and mode come back. |
| ☐ | One instance | Open the app again from the menu | The running window comes forward; no second app. |
| ☐ | Start at login | Turn it on | `~/.config/autostart/unofficial-uniwill-control-center.desktop` exists with `--tray`. |
| ☐ | File dialogs | Settings › Back up…, then Restore… | The desktop's own file dialogs; restoring restarts the app with the settings. |

## Terminal

| | Check | Do | Expect |
|---|---|---|---|
| ☐ | Status | `uucc status` | Mode, readings, battery, lighting; no display line errors. |
| ☐ | Changes | `uucc mode gaming`, `uucc fan-boost on`, `uucc keyboard --effect breathing`, `uucc charge balanced` | Each takes effect and the window follows. |
| ☐ | Display | `uucc display` | "Display color modes aren't available on Linux", exit code 1. |
| ☐ | Not running | Exit the app, `uucc status`; then `uucc status --start` | Exit code 2 with the hint; with `--start` the app starts in the tray and answers. |

## Things that should explain themselves

| | Check | Do | Expect |
|---|---|---|---|
| ☐ | Helper stopped | `sudo systemctl stop uucc-helper.socket uucc-helper.service`, then restart the app | The Performance page says the helper isn't running and how to start it; lighting still works. Start the socket again afterwards. |
| ☐ | Helper restart | With the app open, `sudo systemctl restart uucc-helper.service` | The next change works (the app reconnects). |

Secure Boot detection itself is covered by unit tests (the live USB has Secure Boot off).

## Remove

| | Check | Do | Expect |
|---|---|---|---|
| ☐ | Uninstall | `sudo dnf remove unofficial-uniwill-control-center` | The socket and service are gone (`systemctl status uucc-helper.socket` finds no unit); `~/.config/UnofficialUniwillControlCenter` stays. |

## On Windows, after this release's changes

| | Check | Do | Expect |
|---|---|---|---|
| ☐ | Nothing hidden | Open the Windows app | Display and Plugins pages, Typing/Music/Spectrum tiles, idle timer, touchpad, Windows key and "Start with Windows" are all there. |
| ☐ | Lights | Change keyboard and light bar effects | Both follow, as before. |
| ☐ | uucc | `powershell -ExecutionPolicy Bypass -File scripts\cli-e2e.ps1` | All passed. |
