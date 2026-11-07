# Releasing Notchisland

Releases are built by `.github/workflows/release.yml` on a GitHub macOS runner.
Pushing a tag `vX.Y.Z` produces a GitHub Release with three assets:

| Asset | What it is |
| --- | --- |
| `Notchisland-X.Y.Z.zip` | The universal (Apple Silicon + Intel) `Notchisland.app`, zipped with `ditto -c -k --keepParent` |
| `Notchisland-X.Y.Z.zip.sha256` | `shasum -a 256` of the zip |
| `appcast.json` | `{"version", "build", "url", "sha256", "notes"}` for the in-app updater |

The app bundle contains three executables in `Contents/MacOS`: `notchforge`
(the app), `notchforge-notify` (the CLI agents call) and `notchforge-mcp` (the
MCP server). `packaging/bundle.sh` builds, lipo-merges and signs all of them
from its `BINS` list; add new binaries there.

The in-app updater should poll
`https://github.com/AmdjedBelgacem/Notchify/releases/latest/download/appcast.json`
(GitHub redirects `latest/download/<asset>` to the newest non-prerelease).

## Names that intentionally did not change

The product is called **Notchisland** and the bundle is `Notchisland.app`, but
the bundle identifier `dev.notchforge.NotchForge`, the executable names, the
entitlements file and the data folders (`~/Library/Application Support/NotchForge`,
`~/Library/Logs/NotchForge`) keep their old names. macOS ties privacy
permissions (Accessibility, Calendars, Automation, Full Disk Access) to the
bundle id + code signature; changing them would make every user grant them again.

`INSTALL=1 packaging/bundle.sh` removes an old `/Applications/NotchForge.app`
and points an existing "Launch at login" agent at the new path.

## Cutting a release

1. Bump `version` under `[workspace.package]` in `Cargo.toml` (the workflow
   fails if the tag and that version differ). Commit and push to `main`.
2. Tag and push:
   ```sh
   git tag -a v0.2.0 -m "Short release notes (go into appcast.json)"
   git push origin v0.2.0
   ```
   The tag message becomes `notes` in `appcast.json`; the GitHub Release body
   gets auto-generated notes, which you can edit afterwards. A version with a
   hyphen (`v0.2.0-beta.1`) is published as a prerelease.
3. Watch the **Release** workflow. Without signing secrets it still finishes,
   with a warning, and publishes an ad-hoc-signed zip.
4. Update the Homebrew tap (below).

To rebuild an existing tag, run the workflow by hand (Actions > Release > Run
workflow) and enter the tag. Leaving the tag empty only builds the zip as a
workflow artifact, with no release.

Local equivalent of the CI build: `UNIVERSAL=1 packaging/bundle.sh` (add
`CODESIGN_IDENTITY=… NOTARY_PROFILE=…` to sign and notarize, see the header of
`bundle.sh`).

## Signing and notarization secrets

Optional, and not planned for now: releases ship ad-hoc signed. The steps
below are kept for if that changes.

Without these, users see "Apple could not verify Notchisland is free of
malware" and must right-click > Open (or allow it in System Settings > Privacy
& Security). Every signing step is skipped when a secret is missing.

### One-time Apple setup (needs a paid Apple Developer Program membership)

1. Enroll at <https://developer.apple.com/programs/enroll/> (99 USD/year;
   individual enrollment is fine). Approval can take a day or two.
2. Note your **Team ID**: developer.apple.com > Account > Membership details
   (10 characters, e.g. `AB12CD34EF`).
3. Create a **Developer ID Application** certificate. On your Mac:
   Keychain Access > Certificate Assistant > Request a Certificate From a
   Certificate Authority (save to disk). Then developer.apple.com > Certificates >
   "+" > **Developer ID Application** > upload the request, download the `.cer`
   and double-click it to add it to your login keychain.
4. Export it with its private key: in Keychain Access > My Certificates,
   right-click "Developer ID Application: Your Name (TEAMID)" > Export, save as
   `.p12` with a strong password.
5. Create an **app-specific password** for notarization at
   <https://account.apple.com> > Sign-In and Security > App-Specific Passwords.

### GitHub secrets

Repository > Settings > Secrets and variables > Actions > New repository secret:

| Secret | Value |
| --- | --- |
| `MACOS_CERT_P12_BASE64` | `base64 -i DeveloperID.p12 \| pbcopy`, then paste |
| `MACOS_CERT_PASSWORD` | The `.p12` export password |
| `CODESIGN_IDENTITY` | Exactly `Developer ID Application: Your Name (TEAMID)` (check with `security find-identity -v -p codesigning`) |
| `APPLE_ID` | Your Apple Account email |
| `APPLE_TEAM_ID` | The Team ID |
| `APPLE_APP_PASSWORD` | The app-specific password |

The first three enable signing; the last three enable notarization (only used
when signing is enabled too). Delete the local `.p12` after uploading.

Note: switching from the local self-signed identity
(`packaging/make_signing_identity.sh`) to Developer ID changes the signature,
so permissions have to be granted once more after the first signed install.

## Homebrew tap

The cask is `packaging/homebrew/notchisland.rb`. It is published from a separate
repository so users can run `brew tap AmdjedBelgacem/tap`, `brew trust AmdjedBelgacem/tap` (newer Homebrew refuses untrusted third-party taps) and `brew install --cask notchisland`.

One-time setup:

```sh
gh repo create AmdjedBelgacem/homebrew-tap --public --description "Homebrew tap"
git clone git@github.com:AmdjedBelgacem/homebrew-tap.git ../homebrew-tap
mkdir -p ../homebrew-tap/Casks
```

After each release:

```sh
gh release download v0.2.0 -p 'Notchisland-0.2.0.zip' -D /tmp/nl
packaging/update_cask.sh 0.2.0 /tmp/nl/Notchisland-0.2.0.zip    # updates packaging/homebrew/notchisland.rb
cp packaging/homebrew/notchisland.rb ../homebrew-tap/Casks/notchisland.rb
cd ../homebrew-tap
brew style --cask Casks/notchisland.rb
brew install --cask ./Casks/notchisland.rb   # try it
git commit -am "notchisland 0.2.0" && git push
```

Use the zip from the release, not a local build: the sha256 must match the
published file. Unsigned releases install, but Homebrew marks the app as
quarantined, so the Gatekeeper warning still appears.

## CI checks related to releases

- **Self-test gate**: `packaging/selftest_gate.py` fails CI on any failed UI
  self-test check except those whose name starts with a prefix listed in
  `packaging/selftest_flaky.txt`.
- **Visual regression** (non-blocking): `packaging/visual_diff.py` compares the
  self-test PNGs with `design/baseline/*.png`. See `design/baseline/README.md`
  for refreshing the baseline.

## Shortcuts actions and library validation

`packaging/bundle.sh` builds the native Shortcuts actions
(`packaging/intents/`) into `Contents/Frameworks/libNotchislandIntents.dylib`
plus `Contents/Resources/Metadata.appintents` whenever Xcode is installed (the
GitHub macOS runners have it). Developer ID builds are signed with
`packaging/NotchForge.entitlements`, so library validation stays on and the
library passes because it carries the same Team ID. Local and ad-hoc builds
use `packaging/NotchForge.local.entitlements`, which adds
`com.apple.security.cs.disable-library-validation`: a self-signed identity has
no Team ID, so the app could not load its own library otherwise.
