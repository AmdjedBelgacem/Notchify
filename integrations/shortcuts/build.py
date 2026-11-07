#!/usr/bin/env python3
"""Build the Notchisland shortcuts (unsigned plists in src/), then sign them
with macOS's `shortcuts sign` into this folder. Each one opens a
notchisland:// link (see crates/notchforge-macos/src/url_scheme.rs).

    python3 integrations/shortcuts/build.py
"""
import os, plistlib, subprocess, sys, uuid

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, "src")

def base(actions, color=4282601983, glyph=59511, takes_text=False):
    return {
        "WFWorkflowActions": actions,
        "WFWorkflowClientVersion": "2607.0.2",
        "WFWorkflowMinimumClientVersion": 900,
        "WFWorkflowMinimumClientVersionString": "900",
        "WFWorkflowIcon": {"WFWorkflowIconStartColor": color, "WFWorkflowIconGlyphNumber": glyph},
        "WFWorkflowImportQuestions": [],
        "WFWorkflowInputContentItemClasses": ["WFStringContentItem"] if takes_text else [],
        "WFWorkflowTypes": ["MenuBar"] + (["ActionExtension"] if takes_text else []),
        "WFWorkflowHasShortcutInputVariables": takes_text,
    }

def open_link(url):
    return [
        {"WFWorkflowActionIdentifier": "is.workflow.actions.url",
         "WFWorkflowActionParameters": {"WFURLActionURL": url}},
        {"WFWorkflowActionIdentifier": "is.workflow.actions.openurl",
         "WFWorkflowActionParameters": {}},
    ]

def notify_with_input():
    """Show the shortcut's input text (or what you type) in the notch."""
    ask, enc = str(uuid.uuid4()).upper(), str(uuid.uuid4()).upper()
    prefix = "notchisland://notify?title="
    return [
        {"WFWorkflowActionIdentifier": "is.workflow.actions.ask",
         "WFWorkflowActionParameters": {
             "UUID": ask,
             "WFAskActionPrompt": "What should the notch say?",
             "WFAskActionDefaultAnswer": {
                 "Value": {"Type": "ExtensionInput"}, "WFSerializationType": "WFTextTokenAttachment"}}},
        {"WFWorkflowActionIdentifier": "is.workflow.actions.urlencode",
         "WFWorkflowActionParameters": {
             "UUID": enc,
             "WFInput": {"Value": {"Type": "ActionOutput", "OutputUUID": ask, "OutputName": "Provided Input"},
                         "WFSerializationType": "WFTextTokenAttachment"}}},
        {"WFWorkflowActionIdentifier": "is.workflow.actions.url",
         "WFWorkflowActionParameters": {"WFURLActionURL": {
             "Value": {"string": prefix + "￼",
                       "attachmentsByRange": {"{%d, 1}" % len(prefix): {
                           "Type": "ActionOutput", "OutputUUID": enc, "OutputName": "URL Encoded Text"}}},
             "WFSerializationType": "WFTextTokenString"}}},
        {"WFWorkflowActionIdentifier": "is.workflow.actions.openurl", "WFWorkflowActionParameters": {}},
    ]

SHORTCUTS = {
    "Open Notchisland": base(open_link("notchisland://open")),
    "Notchisland Music": base(open_link("notchisland://tab/music"), glyph=59446),
    "Notchisland Live Sessions": base(open_link("notchisland://tab/live"), glyph=59784),
    "Notchisland Planner": base(open_link("notchisland://tab/planner"), glyph=59784),
    "Focus Timer 25 min": base(open_link("notchisland://timer?minutes=25&label=Focus"), color=4292093695, glyph=59511),
    "Allow Agent Request": base(open_link("notchisland://allow"), color=4282601983, glyph=59443),
    "Deny Agent Request": base(open_link("notchisland://deny"), color=4282601983, glyph=59443),
    "Play or Pause": base(open_link("notchisland://play-pause"), glyph=59446),
    "Next Track": base(open_link("notchisland://next"), glyph=59446),
    "Shelve Clipboard": base(open_link("notchisland://shelve"), glyph=59511),
    "Show in Notchisland": base(notify_with_input(), takes_text=True),
}

def main():
    os.makedirs(SRC, exist_ok=True)
    ok = True
    for name, wf in SHORTCUTS.items():
        src = os.path.join(SRC, name + ".wflow")
        with open(src, "wb") as f:
            plistlib.dump(wf, f, fmt=plistlib.FMT_BINARY)
        out = os.path.join(HERE, name + ".shortcut")
        for _attempt in range(3):  # Apple's signing service is online; retry blips
            r = subprocess.run(["shortcuts", "sign", "--mode", "anyone", "--input", src, "--output", out],
                               capture_output=True, text=True)
            if r.returncode == 0:
                break
        status = "signed" if r.returncode == 0 and os.path.exists(out) else "FAILED: " + (r.stderr.strip() or r.stdout.strip())
        ok &= status == "signed"
        print(f"{name}: {status}")
    sys.exit(0 if ok else 1)

if __name__ == "__main__":
    main()
