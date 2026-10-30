// OpenCode plugin: forward session events to NotchForge.
//
// Install: copy to ~/.config/opencode/plugin/notchforge.js (global) or
// .opencode/plugin/notchforge.js (per project).
const ENDPOINT = process.env.NOTCHFORGE_URL ?? "http://127.0.0.1:47815/v1/notify";

async function notify(body) {
  try {
    await fetch(ENDPOINT, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...(process.env.NOTCHFORGE_TOKEN ? { Authorization: `Bearer ${process.env.NOTCHFORGE_TOKEN}` } : {}),
      },
      body: JSON.stringify({ source: "opencode", ...body }),
      signal: AbortSignal.timeout(2000),
    });
  } catch {
    // NotchForge not running — never break the agent.
  }
}

export const NotchForge = async ({ directory }) => {
  const project = directory?.split("/").pop() ?? "project";
  return {
    event: async ({ event }) => {
      switch (event.type) {
        case "session.idle":
          await notify({ kind: "task_finished", title: `Finished · ${project}`, thread: event.properties?.sessionID });
          break;
        case "session.error":
          await notify({ kind: "error", title: `Error · ${project}`, body: String(event.properties?.error?.message ?? "Session error") });
          break;
        case "permission.updated":
          await notify({ kind: "needs_input", title: `Needs your input · ${project}`, body: event.properties?.title ?? "" });
          break;
      }
    },
  };
};
