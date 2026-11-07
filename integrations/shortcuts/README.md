# Shortcuts

Ready-made shortcuts for Notchisland, signed so anyone can import them:
double-click a `.shortcut` file and choose **Add Shortcut**. Each one opens a
`notchisland://` link, so they work from the menu bar, Spotlight, Siri, a
keyboard shortcut (Shortcut Details › Add Keyboard Shortcut) or automations.

| Shortcut | Does |
|---|---|
| Open Notchisland | Pins the notch open |
| Notchisland Music / Live Sessions / Planner | Opens the notch on that tab |
| Focus Timer 25 min | Starts a 25-minute timer |
| Allow Agent Request / Deny Agent Request | Answers the coding agent's waiting tool request |
| Play or Pause / Next Track | Media keys |
| Shelve Clipboard | Pins the clipboard into the tray |
| Show in Notchisland | Shows text in the notch: what you type, or the input another shortcut passes in |

Rebuild after changing them: `python3 integrations/shortcuts/build.py`
(writes the plists to `src/` and signs them with `shortcuts sign`, which needs
a network connection).

## Native actions

The app also brings its own actions to the Shortcuts app (search
"Notchisland" in the action list): **Open the Notch** (on a tab), **Start a
Notch Timer**, **Show in the Notch**, **Answer Agent Request**, **Play or
Pause**, **Next Track** and **Shelve Clipboard**, plus Siri / Spotlight
phrases such as "Open Notchisland". They are App Intents written in Swift
([`packaging/intents/Intents.swift`](../../packaging/intents/Intents.swift)),
built by `packaging/bundle.sh` into the bundle when Xcode is installed, and
they open the same `notchisland://` links.
