//! Starts `wisp lsp` for .wisp files: the `wisp` on the worktree's PATH.

use zed_extension_api as zed;

struct Wisp;

impl zed::Extension for Wisp {
    fn new() -> Self {
        Wisp
    }

    fn language_server_command(
        &mut self,
        _: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<zed::Command> {
        let command = worktree
            .which("wisp")
            .ok_or("wisp is not on PATH: install the wisp CLI")?;
        Ok(zed::Command {
            command,
            args: vec!["lsp".into()],
            env: Vec::new(),
        })
    }
}

zed::register_extension!(Wisp);
