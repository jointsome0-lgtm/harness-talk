import { spawn, type ChildProcess } from "node:child_process";
import { createInterface } from "node:readline";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

/** Receive htalk notices in an ordinary Pi session. Model/auth stay with Pi. */
export default function (pi: ExtensionAPI) {
  let child: ChildProcess | undefined;
  function stop() {
    const previous = child;
    child = undefined;
    previous?.kill();
  }

  pi.on("session_shutdown", stop);
  pi.on("session_start", (_event, ctx) => {
    stop();
    const peer = process.env.HTALK_PEER;
    if (!peer) return;
    const current = spawn(process.env.HTALK_BIN || "htalk", ["--as", peer, "watch"], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    child = current;
    const fail = (reason: string) => {
      if (child !== current) return;
      stop();
      ctx.ui.notify(`htalk stopped: ${reason}. Check HTALK_PEER, HTALK_DB and htalk watch; reload to reconnect.`, "error");
    };
    current.on("error", () => fail("could not start htalk"));
    current.on("exit", (code) => fail(`watch exited (${code})`));
    // Drain diagnostics without mixing them into Pi's JSON/RPC stdout.
    current.stderr?.resume();
    const lines = createInterface({ input: current.stdout! });
    lines.on("line", (line) => {
      if (child !== current) return;
      try {
        const event = JSON.parse(line);
        if (event.event === "ready") {
          ctx.ui.notify(`htalk listening as ${peer}`, "info");
        } else if (event.event === "message" && typeof event.notification === "string") {
          pi.sendMessage({ customType: "htalk", content: event.notification, display: true },
            { triggerTurn: true, deliverAs: "followUp" });
        } else {
          fail("watch returned an error or unsupported event");
        }
      } catch {
        fail("could not queue a notice");
      }
    });
  });
}
