"""Native configuration for managed Antigravity SDK sessions.

SDK 0.1.18's LocalOpenAIAgentConfig drops continuation mode and policies.
Use its AgentConfig factory seam and forward the required fields explicitly.
"""
from importlib.metadata import version

from google.antigravity import types
from google.antigravity.connections.connection import AgentConfig
from google.antigravity.connections.local.local_openai_connection import LocalOpenAIConnectionStrategy
from google.antigravity.hooks import policy


class ReceiverConfig(AgentConfig):
    model: str
    base_url: str

    def create_strategy(self, *, tool_runner, hook_runner):
        if version("google-antigravity") != "0.1.18":
            raise RuntimeError("This receiver requires SDK 0.1.18")
        return LocalOpenAIConnectionStrategy(
            base_url=self.base_url, model_name=self.model,
            tool_runner=tool_runner, hook_runner=hook_runner,
            capabilities_config=self.capabilities,
            conversation_id=self.conversation_id,
            session_continuation_mode=self.session_continuation_mode,
            save_dir=self.save_dir, app_data_dir=self.app_data_dir,
            workspaces=self.workspaces, mcp_servers=self.mcp_servers,
            policies=list(self.policies), retry_config=self.retry_config,
            # The native process inherits only its isolated SDK host's env.
            env={},
        )


def configuration(root, htalk, db, peer, model, base_url, session_id, creating):
    server = types.McpStdioServer(name="htalk", command=str(htalk),
        args=["--db", str(db), "--as", peer, "mcp"], enabled_tools=["htalk"])
    return ReceiverConfig(model=model, base_url=base_url,
        conversation_id=session_id,
        session_continuation_mode=(types.SessionContinuationMode.CREATE_ONLY if creating
                                   else types.SessionContinuationMode.RESUME),
        save_dir=str(root / "sessions"), app_data_dir=str(root / "app-data"),
        workspaces=[str(root / "work")], mcp_servers=[server],
        capabilities=types.CapabilitiesConfig(enable_subagents=False, enabled_tools=[]),
        policies=[policy.deny_all(), policy.allow(server, ["htalk"])],
        retry_config=types.RetryConfig(
            api_retry=types.ModelAPIRetryConfig(max_retries=0),
            model_output_retry=types.ModelOutputRetryConfig(max_retries=0)))
