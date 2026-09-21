use zed_extension_api::{self as zed, settings::LspSettings};

struct NcExtension;

impl zed::Extension for NcExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<zed::Command> {
        let settings = LspSettings::for_worktree(id.as_ref(), worktree)?;
        let binary = settings.binary.unwrap_or(zed::settings::CommandSettings {
            path: None,
            arguments: None,
            env: None,
        });
        let command = binary.path.or_else(|| worktree.which("ncc")).ok_or(
            "Install ncc with LSP support on PATH, or set lsp.ncc.binary.path in Zed settings",
        )?;
        let mut env = worktree.shell_env();
        if let Some(overrides) = binary.env {
            for (key, value) in overrides {
                env.retain(|(existing, _)| existing != &key);
                env.push((key, value));
            }
        }
        Ok(zed::Command {
            command,
            args: binary.arguments.unwrap_or_else(|| vec!["lsp".into()]),
            env,
        })
    }
}

zed::register_extension!(NcExtension);
