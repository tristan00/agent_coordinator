use crate::Result;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::{env, fs};

pub const VARIABLE: &str = "COORDINATOR_KILL_GUARD";
const TOOLS: [&str; 3] = ["taskkill", "pkill", "killall"];

pub fn tool() -> Option<&'static str> {
    let exe = env::current_exe().ok()?;
    let stem = exe.file_stem()?.to_str()?.to_ascii_lowercase();
    TOOLS.into_iter().find(|name| *name == stem)
}

pub fn refusal(tool: &str, args: &[String]) -> Option<String> {
    let broad = match tool {
        "taskkill" => {
            let flags: Vec<String> = args.iter().map(|arg| arg.trim_start_matches(['/', '-']).to_ascii_lowercase()).collect();
            flags.iter().enumerate().any(|(i, flag)| flag == "im" || (flag == "fi" && !args.get(i + 1).is_some_and(|f| f.trim().to_ascii_lowercase().starts_with("pid"))))
        }
        _ => true,
    };
    broad.then(|| {
        format!("{tool} {}: refused by the agent kill guard. Killing by name or filter also stops other agents' processes on this machine. Kill the specific PID you started instead.", args.join(" "))
    })
}

pub fn run(tool: &str) -> ! {
    let args: Vec<String> = env::args().skip(1).collect();
    if let Some(reason) = refusal(tool, &args) {
        eprintln!("{reason}");
        std::process::exit(1);
    }
    let guard = env::var_os(VARIABLE).map(PathBuf::from);
    let real = env::var_os("PATH").and_then(|path| {
        env::split_paths(&path).filter(|dir| Some(dir) != guard.as_ref()).find_map(|dir| {
            [dir.join(tool), dir.join(format!("{tool}{}", env::consts::EXE_SUFFIX))].into_iter().find(|file| file.is_file())
        })
    });
    let Some(real) = real else {
        eprintln!("{tool}: command not found");
        std::process::exit(127);
    };
    match Command::new(real).args(&args).status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("{tool}: {error}");
            std::process::exit(126);
        }
    }
}

pub fn install(data: &Path) -> Result<PathBuf> {
    let exe = env::current_exe()?;
    let digest = crate::evidence::digest(&exe)?;
    let dir = data.join("kill-guard").join(&digest.trim_start_matches("sha256:")[..16]);
    fs::create_dir_all(&dir)?;
    for name in TOOLS {
        let link = dir.join(format!("{name}{}", env::consts::EXE_SUFFIX));
        if !link.is_file() && fs::hard_link(&exe, &link).is_err() && !link.is_file() {
            fs::copy(&exe, &link)?;
        }
    }
    let check = dir.join("shell-check.txt");
    if !check.exists() {
        let report = shell_check(&dir);
        if report.contains("BYPASSED") {
            eprintln!("Kill guard is bypassed by some shells; see {}", check.display());
        }
        fs::write(&check, report)?;
    }
    Ok(dir)
}

pub fn environment(dir: &Path) -> Result<[(&'static str, OsString); 2]> {
    let path = env::join_paths(std::iter::once(dir.to_path_buf()).chain(env::var_os("PATH").iter().flat_map(env::split_paths)))?;
    Ok([("PATH", path), (VARIABLE, dir.as_os_str().to_owned())])
}

fn shell_check(dir: &Path) -> String {
    let probes: &[(&str, &[&str], &str)] = &[
        ("bash", &["-c"], "command -v {tool}"),
        ("bash", &["-lc"], "command -v {tool}"),
        ("sh", &["-c"], "command -v {tool}"),
        ("zsh", &["-lc"], "command -v {tool}"),
        ("pwsh", &["-NoProfile", "-Command"], "(Get-Command {tool} -CommandType Application | Select-Object -First 1).Source"),
        ("powershell", &["-NoProfile", "-Command"], "(Get-Command {tool} -CommandType Application | Select-Object -First 1).Source"),
        ("cmd", &["/d", "/c"], "where {tool}"),
    ];
    let Ok(vars) = environment(dir) else { return "Could not build PATH\n".into() };
    let marker = dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let mut report = String::new();
    for (shell, flags, script) in probes {
        for tool in TOOLS {
            let mut command = Command::new(shell);
            command.args(*flags).arg(script.replace("{tool}", tool)).envs(vars.clone()).stdin(Stdio::null()).stderr(Stdio::null());
            let Ok(output) = command.output() else { break };
            let found = String::from_utf8_lossy(&output.stdout).lines().next().unwrap_or("").trim().to_owned();
            let verdict = if found.is_empty() { "absent" } else if found.contains(&marker) { "guarded" } else { "BYPASSED" };
            report.push_str(&format!("{shell} {} {tool}: {verdict} {found}\n", flags.join(" ")));
        }
    }
    report
}

#[cfg(test)]
#[path = "kill_guard_tests.rs"]
mod tests;
