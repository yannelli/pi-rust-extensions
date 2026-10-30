use std::path::{Path, PathBuf};

use pi_ext_sdk::prelude::*;
use tokio::process::Command;

use crate::config::{Config, Profile};

pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn socket_path(agent_dir: &Path, profile: &Profile) -> Result<PathBuf> {
    let connection = json!([
        profile.host,
        profile.user,
        profile.port,
        profile.identity_file,
        profile.jump_host,
        profile.options
    ]);
    let hash = connection
        .to_string()
        .bytes()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
    let directory = agent_dir.join("ext/ssh/sockets");
    std::fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(directory.join(format!("{hash:016x}")))
}

pub fn arguments(agent_dir: &Path, config: &Config, profile: &Profile) -> Result<Vec<String>> {
    let socket = socket_path(agent_dir, profile)?;
    let mut args = vec![
        "-T".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ControlMaster=auto".into(),
        "-o".into(),
        format!("ControlPersist={}", config.control_persist),
        "-o".into(),
        format!(
            "ControlPath={}",
            socket.file_name().unwrap().to_string_lossy()
        ),
        "-o".into(),
        "ConnectTimeout=15".into(),
        "-p".into(),
        profile.port.to_string(),
    ];
    for (flag, value) in [
        ("-l", &profile.user),
        ("-i", &profile.identity_file),
        ("-J", &profile.jump_host),
    ] {
        if let Some(value) = value {
            args.extend([flag.into(), value.clone()]);
        }
    }
    for (key, value) in &profile.options {
        args.extend(["-o".into(), format!("{key}={value}")]);
    }
    Ok(args)
}

pub fn remote_command(profile: &Profile, command: &str, cwd: Option<&str>) -> String {
    let mut script = String::new();
    if let Some(cwd) = cwd.or(profile.cwd.as_deref()) {
        script.push_str(&format!("cd -- {} || exit $?; ", quote(cwd)));
    }
    for (key, value) in &profile.env {
        script.push_str(&format!("export {key}={}; ", quote(value)));
    }
    script.push_str(&format!("exec sh -c {}", quote(command)));
    format!("sh -c {}", quote(&script))
}

pub fn build(
    agent_dir: &Path,
    config: &Config,
    profile: &Profile,
    remote: &str,
) -> Result<Command> {
    let mut command = Command::new("ssh");
    command
        .current_dir(agent_dir.join("ext/ssh/sockets"))
        .args(arguments(agent_dir, config, profile)?)
        .arg(&profile.host)
        .arg(remote);
    Ok(command)
}

pub fn shell_line(
    agent_dir: &Path,
    config: &Config,
    profile: &Profile,
    remote: &str,
) -> Result<String> {
    let mut args = vec!["ssh".into()];
    args.extend(arguments(agent_dir, config, profile)?);
    args.extend([profile.host.clone(), remote.into()]);
    Ok(format!(
        "cd -- {} && exec {}",
        quote(&agent_dir.join("ext/ssh/sockets").to_string_lossy()),
        args.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" ")
    ))
}

pub fn profile<'a>(config: &'a Config, name: &str) -> Result<&'a Profile> {
    let profile = config
        .profiles
        .get(name)
        .ok_or_else(|| Error::msg(format!("Unknown SSH profile: {name}")))?;
    if !profile.enabled {
        return Err(Error::msg(format!("SSH profile is disabled: {name}")));
    }
    Ok(profile)
}
