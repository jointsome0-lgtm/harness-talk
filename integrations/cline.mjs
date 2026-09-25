/** Attach-only receiver for Cline CLI 3.0.65 / @cline/core 0.0.86. */
import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";

const { HTALK_PEER: peer, HTALK_DB: db, HTALK_SESSION: sessionId, HTALK_WORKSPACE: workspace,
  HTALK_CLINE_ROOT: clineRoot } = process.env;
const emit = (event, values = {}) => process.stdout.write(JSON.stringify({ event, ...values }) + "\n");
let client, watcher, lines, stopping = false, boundClient;
async function stop() {
  stopping = true;
  lines?.close();
  if (watcher && watcher.exitCode === null && watcher.signalCode === null) {
    watcher.kill();
    const timer = setTimeout(() => watcher.kill("SIGKILL"), 2000);
    timer.unref();
    await new Promise(resolve => watcher.once("close", resolve));
    clearTimeout(timer);
  }
  await client?.dispose();
}
for (const signal of ["SIGINT", "SIGTERM"]) process.once(signal, () => void stop());

async function attached() {
  const [sessions, clients] = await Promise.all([
    client.command("session.list"), client.command("client.list"),
  ]);
  if (!sessions.ok || !clients.ok) throw Error("hub_read_failed");
  const session = sessions.payload?.sessions?.find(s => s.sessionId === sessionId);
  if (!session || session.workspaceRoot !== workspace ||
      !["idle", "running"].includes(session.status)) throw Error("session_unavailable");
  const participants = new Set(session.participants?.map(p => p.clientId));
  const terminals = clients.payload?.clients?.filter(c => c.clientType === "cli" &&
    participants.has(c.clientId) && c.workspaceContext?.workspaceRoot === workspace) ?? [];
  if (terminals.length !== 1) throw Error("require_one_attached_terminal");
  if (!boundClient) {
    boundClient = terminals[0].clientId;
  } else if (!terminals.some(c => c.clientId === boundClient)) {
    throw Error("terminal_detached");
  }
}

try {
  if (!peer || !db || !sessionId || !workspace || !clineRoot) throw Error("missing_receiver_configuration");
  const core = resolve(clineRoot, "node_modules/@cline/core");
  if (JSON.parse(readFileSync(resolve(core, "package.json"), "utf8")).version !== "0.0.86")
    throw Error("unsupported_cline_core_version");
  const modulePath = resolve(core, "dist/hub/index.js");
  const hub = await import(pathToFileURL(modulePath).href);
  const discovery = await hub.readHubDiscovery(hub.resolveProductionHubOwnerContext().discoveryPath);
  if (stopping) throw Error("stopped");
  if (!discovery) throw Error("hub_not_running");
  const url = new URL(discovery.url);
  if (url.protocol !== "ws:" || !["127.0.0.1", "localhost", "[::1]"].includes(url.hostname) ||
      url.username || url.password || url.search || url.hash) throw Error("require_local_hub");
  // No ensure/resolve helper: they can launch or replace a hub during recovery.
  client = new hub.NodeHubClient({ url: discovery.url, authToken: discovery.authToken,
    clientType: "htalk-receiver", workspaceRoot: workspace, cwd: workspace });
  await client.connect();
  await attached();
  if (stopping) throw Error("stopped");
  client.subscribe(event => {
    if (stopping) return;
    if (event.payload?.clientId === boundClient &&
        (event.event === "hub.client.disconnected" ||
         (event.event === "session.detached" && event.sessionId === sessionId))) {
      emit("stopped", { reason: "terminal_detached" });
      void stop();
    }
  });
  watcher = spawn(process.env.HTALK_BIN || "htalk", ["--db", db, "--as", peer, "watch"], {
    stdio: ["ignore", "pipe", "pipe"],
  });
  let watchError;
  watcher.on("error", () => { watchError = "watch_start_failed"; lines?.close(); });
  watcher.stderr.resume();
  lines = createInterface({ input: watcher.stdout });
  for await (const line of lines) {
    if (stopping) break;
    const event = JSON.parse(line);
    if (event.event === "ready" && event.peer === peer) {
      emit("ready", { peer, session_id: sessionId });
      continue;
    }
    if (event.event !== "message" || typeof event.id !== "string" ||
        typeof event.notification !== "string") throw Error("invalid_watch_event");
    await attached();
    let dispatched = false;
    try {
      const reply = await client.command("run.enqueue", { prompt: event.notification, delivery: "queue" }, sessionId, {
        timeoutMs: 10000,
        beforeDispatch() {
          // The SDK retries transport errors. A second enqueue is unsafe.
          if (dispatched || stopping) throw Error("refuse_repeat_dispatch");
          dispatched = true;
        },
      });
      if (!reply.ok) {
        emit("notice", { id: event.id, submission: "submission_unknown" });
        throw Error("enqueue_rejected");
      }
      emit("notice", { id: event.id, submission: "submitted", run_id: reply.payload?.runId });
    } catch (error) {
      if (error.message !== "enqueue_rejected")
        emit("notice", { id: event.id, submission: dispatched ? "submission_unknown" : "not_submitted" });
      throw error;
    }
  }
  if (!stopping) throw Error(watchError || "watch_exited");
} catch (error) {
  // SDK errors can contain connection details. Credentials never enter receipts.
  const reason = /^[a-z_]+$/.test(error.message) ? error.message : "receiver_failed";
  emit("error", { reason, message: "Receiver stopped. Check the peer, live terminal, hub and watch command; inspect saved mail before restarting." });
  process.exitCode = 1;
} finally {
  await stop();
}
