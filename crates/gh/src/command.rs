use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;
use std::process::{Command, Output};

pub(crate) fn run_gh(args: &[&str]) -> Result<String> {
    checked_output(Command::new("gh").args(args).output(), "gh", args)
}

pub(crate) fn run_git(repo: &Path, args: &[&str]) -> Result<String> {
    checked_output(
        Command::new("git").arg("-C").arg(repo).args(args).output(),
        "git",
        args,
    )
}

fn checked_output(output: std::io::Result<Output>, program: &str, args: &[&str]) -> Result<String> {
    let output = output.map_err(|err| anyhow!("failed to run {program}: {err}"))?;
    if !output.status.success() {
        bail!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context(format!("{program} output was not UTF-8"))
}
