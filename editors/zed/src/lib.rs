use zed_extension_api::{self as zed, Command, LanguageServerId, Result, Worktree};

struct MangroveExtension;

impl zed::Extension for MangroveExtension {
    fn new() -> Self {
        MangroveExtension
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Command> {
        let command = worktree
            .which("mangrove")
            .ok_or_else(|| "mangrove binary not found on PATH — install it with `cargo install --path crates/mangrove-cli`".to_string())?;

        Ok(Command {
            command,
            args: vec!["lsp".to_string()],
            env: vec![],
        })
    }
}

zed::register_extension!(MangroveExtension);
