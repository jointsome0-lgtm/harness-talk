import { spawn, type ChildProcess } from "node:child_process";
import { createInterface, type Interface } from "node:readline";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

/** Receive htalk notices in an ordinary Pi session. Model/auth stay with Pi. */
export default function (pi: ExtensionAPI) {
  let receiver: { child: ChildProcess; lines: Interface; closed: Promise<void> } | undefined;
  let closing: typeof receiver;
  let stopPromise: Promise<void> | undefined;
  let generation = 0;
  function stop(): Promise<void> {
    if (stopPromise) return stopPromise;
    const previous = receiver;
    receiver = undefined;
    if (!previous) return Promise.resolve();
    closing = previous;
    previous.lines.close();
    stopPromise = (async () => {
      const child = previous.child;
      let deadline: ReturnType<typeof setTimeout> | undefined;
      let timeout: ReturnType<typeof setTimeout> | undefined;
      try {
        if (child.exitCode !== null || child.signalCode !== null) {
          child.stdout?.destroy();
          child.stderr?.destroy();
        }
        // Node emits close after exit or a spawn error and after closing pipes.
        await Promise.race([previous.closed, new Promise<never>((_resolve, reject) => {
          timeout = setTimeout(() => {
            child.stdout?.destroy();
            child.stderr?.destroy();
            reject(new Error("htalk watcher did not close within 4 seconds; replacement refused"));
          }, 4000);
          timeout.unref();
          if (child.pid && child.exitCode === null && child.signalCode === null) {
            child.kill();
            deadline = setTimeout(() => child.kill("SIGKILL"), 2000);
            deadline.unref();
          }
        })]);
        closing = undefined;
      } finally {
        clearTimeout(deadline);
        clearTimeout(timeout);
      }
    })().then(() => { stopPromise = undefined; });
    // On failure retain the rejected operation and child. Future starts fail
    // instead of replacing a process whose termination was not confirmed.
    return stopPromise;
  }

  pi.on("session_shutdown", () => {
    generation++;
    return stop();
  });
  pi.on("session_start", async (_event, ctx) => {
    const starting = ++generation;
    await stop();
    if (starting !== generation) return;
    const peer = process.env.HTALK_PEER;
    if (!peer) return;
    const current = spawn(process.env.HTALK_BIN || "htalk", ["--as", peer, "watch"], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    const closed = new Promise<void>(resolve => current.once("close", () => resolve()));
    current.on("exit", () => {
      if (closing?.child === current) {
        // A descendant can hold inherited pipe handles after this child exits.
        current.stdout?.destroy();
        current.stderr?.destroy();
      }
    });
    const lines = createInterface({ input: current.stdout! });
    receiver = { child: current, lines, closed };
    const fail = (reason: string) => {
      if (receiver?.child !== current) return;
      void stop().catch(() => {
        ctx.ui.notify("htalk watcher cleanup timed out; replacement refused.", "error");
      });
      ctx.ui.notify(`htalk stopped: ${reason}. Check HTALK_PEER, HTALK_DB and htalk watch; reload to reconnect.`, "error");
    };
    current.on("error", () => fail("could not start htalk"));
    current.on("exit", (code) => fail(`watch exited (${code})`));
    // Drain diagnostics without mixing them into Pi's JSON/RPC stdout.
    current.stderr?.resume();
    lines.on("line", (line) => {
      if (receiver?.child !== current) return;
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
