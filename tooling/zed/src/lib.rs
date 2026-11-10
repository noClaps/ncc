//! Zed extension for NC: launches `ncc lsp` for `.nc` files.

use zed_extension_api::{self as zed, LanguageServerId, Result};

struct NcExtension;

impl zed::Extension for NcExtension {
    fn new() -> Self {
        NcExtension
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let path = worktree.which("ncc").ok_or_else(|| {
            "`ncc` was not found on PATH; install it to enable the NC language server".to_string()
        })?;
        Ok(zed::Command {
            command: path,
            args: vec!["lsp".into()],
            env: worktree.shell_env(),
        })
    }
}

zed::register_extension!(NcExtension);
