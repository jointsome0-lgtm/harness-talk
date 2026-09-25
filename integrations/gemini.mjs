/** Ordinary Gemini CLI 0.61.0 launcher using its internal TUI injection queue. */
import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";

const { HTALK_PEER: peer, HTALK_DB: database, HTALK_GEMINI_ROOT: root } = process.env;
if (!peer || !database || !root) throw Error("Set HTALK_PEER, HTALK_DB and HTALK_GEMINI_ROOT.");
if (JSON.parse(readFileSync(resolve(root, "package.json"), "utf8")).version !== "0.61.0")
  throw Error("This htalk launcher requires Gemini CLI 0.61.0; check its TUI interfaces before upgrading.");

const load = name => import(pathToFileURL(resolve(root, "bundle", name)).href);
const { Config, coreEvents } = await load("chunk-JDPZ4CE3.js");
const { registerCleanup, runExitCleanup } = await load("cleanup-XO3PUOXN.js");
let watcher, lines, boundConfig, stopping = false, stopPromise;

function stop() {
  if (stopPromise) return stopPromise;
  stopping = true;
  lines?.close();
  stopPromise = (async () => {
    if (watcher && watcher.exitCode === null && watcher.signalCode === null) {
      watcher.kill();
      const timer = setTimeout(() => watcher.kill("SIGKILL"), 2000);
      timer.unref();
      await new Promise(resolve => watcher.once("close", resolve));
      clearTimeout(timer);
    }
  })();
  return stopPromise;
}
registerCleanup(stop);

async function receive(config) {
  try {
    if (!config.isModelSteeringEnabled())
      throw Error("Enable experimental.modelSteering in Gemini settings before starting this receiver.");
    watcher = spawn(process.env.HTALK_BIN || "htalk", ["--db", database, "--as", peer, "watch"],
      { stdio: ["ignore", "pipe", "pipe"] });
    watcher.on("error", () => lines?.close());
    watcher.stderr.resume();
    lines = createInterface({ input: watcher.stdout });
    for await (const line of lines) {
      if (stopping) break;
      const event = JSON.parse(line);
      if (event.event === "ready" && event.peer === peer) {
        coreEvents.emitFeedback("info", `htalk listening as ${peer}`);
        continue;
      }
      if (event.event !== "message" || typeof event.notification !== "string")
        throw Error("Unexpected htalk watch event.");
      // The stock TUI holds background completions until idle, MCP ready and
      // no pending approval. It owns display, user drafts and model invocation.
      config.injectionService.addInjection(event.notification, "background_completion");
    }
    if (!stopping) throw Error("htalk watch exited; check the peer and database.");
  } catch (error) {
    coreEvents.emitFeedback("error", `htalk receiver stopped. ${error.message}`);
  } finally {
    await stop();
  }
}

const initialize = Config.prototype.initialize;
Config.prototype.initialize = async function (...args) {
  const result = await initialize.apply(this, args);
  if (this.isInteractive() && !boundConfig && !stopping) {
    boundConfig = this;
    void receive(this);
  }
  return result;
};

try {
  const { main } = await load("gemini-3HFX2LNT.js");
  await main();
} catch (error) {
  await runExitCleanup();
  process.stderr.write(`Gemini launcher failed: ${error.message}\n`);
  process.exitCode = 1;
} finally {
  Config.prototype.initialize = initialize;
  await stop();
}
