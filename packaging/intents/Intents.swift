// Native Shortcuts actions for Notchisland (App Intents).
//
// Built by packaging/intents/build.sh into the app bundle as
// Contents/Frameworks/libNotchislandIntents.dylib, with the action metadata in
// Contents/Resources/Metadata.appintents. The app loads the library at launch.
// Every action opens a notchisland:// link, so the Rust side handles them all
// in one place (crates/notchforge-macos/src/url_scheme.rs).

import AppIntents
import AppKit

private func open(_ link: String) {
    guard let url = URL(string: link) else { return }
    DispatchQueue.main.async { NSWorkspace.shared.open(url) }
}

private func encoded(_ s: String) -> String {
    var allowed = CharacterSet.urlQueryAllowed
    allowed.remove(charactersIn: "&=+?#")
    return s.addingPercentEncoding(withAllowedCharacters: allowed) ?? ""
}

enum NotchTab: String, AppEnum {
    case overview, planner, music, live, agents

    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Notch Tab"
    static let caseDisplayRepresentations: [NotchTab: DisplayRepresentation] = [
        .overview: "Overview",
        .planner: "Planner",
        .music: "Music",
        .live: "Live Sessions",
        .agents: "Agents",
    ]
}

enum AgentAnswer: String, AppEnum {
    case allow, deny

    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Answer"
    static let caseDisplayRepresentations: [AgentAnswer: DisplayRepresentation] = [
        .allow: "Allow",
        .deny: "Deny",
    ]
}

struct OpenNotchIntent: AppIntent {
    static let title: LocalizedStringResource = "Open the Notch"
    static let description = IntentDescription("Opens Notchisland's panel on a tab.")

    @Parameter(title: "Tab", default: .overview)
    var tab: NotchTab

    static var parameterSummary: some ParameterSummary {
        Summary("Open the notch on \(\.$tab)")
    }

    func perform() async throws -> some IntentResult {
        open("notchisland://tab/\(tab.rawValue)")
        return .result()
    }
}

struct StartNotchTimerIntent: AppIntent {
    static let title: LocalizedStringResource = "Start a Notch Timer"
    static let description = IntentDescription("Starts a timer that counts down in the notch.")

    @Parameter(title: "Minutes", default: 25, inclusiveRange: (1, 1440))
    var minutes: Int

    @Parameter(title: "Label", default: "Focus")
    var label: String

    static var parameterSummary: some ParameterSummary {
        Summary("Start a \(\.$minutes)-minute timer called \(\.$label)")
    }

    func perform() async throws -> some IntentResult {
        open("notchisland://timer?minutes=\(minutes)&label=\(encoded(label))")
        return .result()
    }
}

struct ShowInNotchIntent: AppIntent {
    static let title: LocalizedStringResource = "Show in the Notch"
    static let description = IntentDescription("Shows a notification in the notch.")

    @Parameter(title: "Title")
    var text: String

    @Parameter(title: "Details", default: "")
    var details: String

    static var parameterSummary: some ParameterSummary {
        Summary("Show \(\.$text) in the notch") {
            \.$details
        }
    }

    func perform() async throws -> some IntentResult {
        open("notchisland://notify?title=\(encoded(text))&body=\(encoded(details))")
        return .result()
    }
}

struct AnswerAgentRequestIntent: AppIntent {
    static let title: LocalizedStringResource = "Answer Agent Request"
    static let description = IntentDescription("Allows or denies the coding agent's tool request waiting in the notch.")

    @Parameter(title: "Answer", default: .allow)
    var answer: AgentAnswer

    static var parameterSummary: some ParameterSummary {
        Summary("\(\.$answer) the waiting agent request")
    }

    func perform() async throws -> some IntentResult {
        open("notchisland://\(answer.rawValue)")
        return .result()
    }
}

struct PlayPauseIntent: AppIntent {
    static let title: LocalizedStringResource = "Play or Pause"
    static let description = IntentDescription("Plays or pauses what the notch is showing.")

    func perform() async throws -> some IntentResult {
        open("notchisland://play-pause")
        return .result()
    }
}

struct NextTrackIntent: AppIntent {
    static let title: LocalizedStringResource = "Next Track"

    func perform() async throws -> some IntentResult {
        open("notchisland://next")
        return .result()
    }
}

struct ShelveClipboardIntent: AppIntent {
    static let title: LocalizedStringResource = "Shelve Clipboard"
    static let description = IntentDescription("Pins the clipboard's text or link into the notch's tray.")

    func perform() async throws -> some IntentResult {
        open("notchisland://shelve")
        return .result()
    }
}

struct NotchislandShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: OpenNotchIntent(),
            phrases: ["Open \(.applicationName)", "Show \(.applicationName) on \(\.$tab)"],
            shortTitle: "Open the Notch",
            systemImageName: "rectangle.topthird.inset.filled"
        )
        AppShortcut(
            intent: StartNotchTimerIntent(),
            phrases: ["Start a \(.applicationName) timer"],
            shortTitle: "Start Timer",
            systemImageName: "timer"
        )
        AppShortcut(
            intent: AnswerAgentRequestIntent(),
            phrases: ["\(\.$answer) the agent in \(.applicationName)"],
            shortTitle: "Answer Agent",
            systemImageName: "checkmark.shield"
        )
        AppShortcut(
            intent: ShelveClipboardIntent(),
            phrases: ["Shelve the clipboard in \(.applicationName)"],
            shortTitle: "Shelve Clipboard",
            systemImageName: "tray.and.arrow.down"
        )
    }
}
