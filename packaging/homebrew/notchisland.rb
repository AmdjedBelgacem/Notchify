# Homebrew cask for Notchisland. Lives in the tap repo
# github.com/AmdjedBelgacem/homebrew-tap as Casks/notchisland.rb.
# packaging/update_cask.sh VERSION ZIP fills in version and sha256.
cask "notchisland" do
  version "0.0.0"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"

  url "https://github.com/AmdjedBelgacem/Notchify/releases/download/v#{version}/Notchisland-#{version}.zip"
  name "Notchisland"
  desc "Dynamic island for the MacBook notch: media, agents, notifications and more"
  homepage "https://github.com/AmdjedBelgacem/Notchify"

  livecheck do
    url :url
    strategy :github_latest
  end

  # The app checks the latest release's appcast.json itself.
  auto_updates true
  depends_on macos: ">= :ventura"

  app "Notchisland.app"
  binary "#{appdir}/Notchisland.app/Contents/MacOS/notchforge-notify"

  uninstall quit:      "dev.notchforge.NotchForge",
            launchctl: "dev.notchforge.NotchForge"

  zap trash: [
    "~/Library/Application Support/NotchForge",
    "~/Library/LaunchAgents/dev.notchforge.NotchForge.plist",
    "~/Library/Logs/NotchForge",
    "~/Library/Preferences/dev.notchforge.NotchForge.plist",
  ]
end
