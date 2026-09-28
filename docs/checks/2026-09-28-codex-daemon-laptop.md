# Codex daemon delivery and laptop route

Checked on 2026-09-28 UTC. The 0.9.4 release fixes a receiver failure observed
with Codex CLI 0.157.1 and a running local app-server daemon 0.158.0. The CLI
refused its `queue` command with the receiver's pinned `sqlite_home` override.
The failure occurred before a queue write. Removing the override would have
removed the store-selection protection added in 0.9.3.

The receiver can now select the server explicitly with `--codex-socket`.
It verifies the exact loaded UUID and workspace on the same connection used
for `thread/queue/add`, and checks that the receipt carries the original htalk
message ID. The existing local store/session lock remains held. This does not
establish a lease across separate servers or prove which database the server
opened. The selected Unix listener is the delivery authority.

The saved binding includes its path, device, inode and change time. A changed
or unavailable listener stops delivery without falling back to the CLI. One
new Rust test exercises real receiver commands against a controlled socket
server, retained receipts, removal of the binding, listener replacement and
an unavailable listener. It also makes any CLI fallback fail visibly.

## Observed work and recovery

A source-built 0.9.4 candidate delivered one bounded computation to a dedicated
GPT-6 Luna medium session on another Linux device. The agent read a CSV with
8192 rows and 17 groups, created an exclusive execution marker and returned a
correlated reply. The controller independently recomputed the result using a
different algorithm. Input and result bytes matched, and the selected native
trace contained one computation command.

The earlier failed notification was preserved as unresolved. An operator
reconciled it only after reproducing the prequeue refusal and inspecting both
native queues, the exact session history, absent computation artifacts and a
clean idle exit. The original request ID and receipt directory were retained.
This was explicit recovery of an observed failure, not an automatic retry.

One delivered turn then reported an unavailable MCP tool and performed no
mail or computation action. The remote session had not retained the
command-line MCP configuration. A dedicated project configuration restored the
tool; a server tool call checked it before an explicit continuation of the same
request. This failed turn remains separate from the successful computation.

After completion, the controller terminated only its dedicated SSH service's
main process. systemd restored the reverse tunnel. A real mailbox read worked
again; the same model and receiver processes, completed-turn count and saved
receipt remained unchanged. The reconnect caused no additional model turn.

## Publication wheel

The x86-64 wheel from [publication run 36429231142](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36429231142)
was checked before upload approval, with commit `cab29eabc7d2671bdb9dfacfddb4ad6303459a05`
and SHA-256 `89494cd0b9b7e6f18cdb3fd13fff7780df0e90e89347979bde8bb0a994844649`.
Both Linux architectures passed build and installed-wheel checks. Every one
of the 97 included Git files matched the release tree.

The exact wheel then delivered a new read-only request through the bound
server socket to the same laptop session. Luna read the existing result and
returned its SHA-256 in a correlated reply. The controller read before ACK
and independently confirmed unchanged input, result, execution marker and
analysis script bytes. Earlier receipts and the socket binding were retained.
The original computation was not repeated for this wheel check.

## Limits

The installed user services were enabled and their controlled SSH restart was
checked. A physical Wi-Fi outage, sleep and reboot were not tested. If a daemon
restart replaces its listener, the saved binding rejects it until inspection
and explicit rebinding. A stopped worker needs explicit same-session recovery;
htalk does not run a model on a timer. The SSH agent may need the owner's login
and unlock after boot. Enabled units alone do not establish unattended startup.

These observations cover Linux LAN transport. Bluetooth, internet routing and
native macOS/Windows operation remain separate work. Mailbox schema stays 3;
no migration is needed from 0.9.3. Queue submission, ACK, reply and an independently
verified result remain separate evidence.
