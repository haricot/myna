# Myna for Noctalia 5.6.0

Local bar widget + panel for **haricot/myna**. No change to the Candle
provider, SmartTurn, dictation settings, or existing Niri keyboard shortcut.

The plugin uses fixed-argument `systemctl --user` calls to read and control:
- `myna-desktop.service`
- `myna-candle-provider.service`

It never uses sudo, a shell interpreter, or an arbitrary command string.
It neither enables nor disables systemd autostart; the menu only starts and
stops the running units. Starting includes both units; stopping stops both.
Myna's systemd unit dependency and ordering are retained. No separate daemon
is created by the plugin.

## Install from the existing Myna checkout

Update the `feature/myna-candle-native-anchors-r1` branch, then in Niri +
Noctalia 5.6.0:

```sh
cd ~/tmp/myna-candle-client-native
git pull --ff-only
noctalia msg plugins source add haricot-myna path "$PWD/extensions/noctalia"
noctalia msg plugins enable haricot/myna
noctalia msg panel-toggle haricot/myna:panel
```

To display the status indicator, use Noctalia Settings → Bar → Add widget →
**Myna**, identified as `haricot/myna:status`. Clicking its microphone icon
toggles `haricot/myna:panel`. The panel is attached near the bar widget.

The menu offers **Démarrer Myna**, **Arrêter Myna**, and **Actualiser l'état**.
Status is polled asynchronously every 3.5 seconds while the widget or panel
is running and immediately after a service action. An inactive service is not
treated as a subprocess failure; the two lines from `systemctl is-active`
are interpreted separately.

Direct terminal verification:

```sh
systemctl --user is-active myna-desktop.service myna-candle-provider.service
journalctl --user -u myna-desktop.service -u myna-candle-provider.service -n 30 --no-pager
```

Notes:
- The menu does **not** trigger the dictation toggle; keep the existing Niri
  keyboard shortcut bound to `myna-desktop --toggle`.
- Noctalia scripts are trusted user-code. Enable this plugin only from your
  own reviewed checkout.
- This is not yet verified in a running Niri/Noctalia graphical session.
- Nothing is started simply by enabling the widget.
