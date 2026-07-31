use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;
use std::process::{Command, Output};

pub(crate) fn run_gh(args: &[&str]) -> Result<String> {
    let mut command = Command::new("gh");
    command.args(args);
    utf8(checked(command, &format!("gh {}", args.join(" ")))?, "gh")
}

pub(crate) fn run_git(repo: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo).args(args);
    utf8(checked(command, &format!("git {}", args.join(" ")))?, "git")
}

pub(crate) fn spawn(mut command: Command, label: &str) -> Result<Output> {
    command.output().map_err(|err| anyhow!("failed to run {label}: {err}"))
}

pub(crate) fn checked(command: Command, label: &str) -> Result<Output> {
    let output = spawn(command, label)?;
    if !output.status.success() {
        bail!("{label} failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(output)
}

fn utf8(output: Output, label: &str) -> Result<String> {
    String::from_utf8(output.stdout).context(format!("{label} output was not UTF-8"))
}
