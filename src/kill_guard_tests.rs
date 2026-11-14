use super::*;

fn args(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_owned).collect()
}

#[test]
fn taskkill_by_name_or_broad_filter_is_refused() {
    for command in ["/F /IM python.exe", "//F //IM node.exe", "/im python.exe /t", "/F /FI IMAGENAME=python.exe", "/FI STATUS=running"] {
        assert!(refusal("taskkill", &args(command)).is_some(), "{command}");
    }
}

#[test]
fn taskkill_by_pid_is_allowed() {
    for command in ["/PID 1234", "/F /PID 1234 /T", "//F //PID 99", "/F /FI PID=1234"] {
        assert_eq!(refusal("taskkill", &args(command)), None, "{command}");
    }
}

#[test]
fn pkill_and_killall_are_always_refused() {
    assert!(refusal("pkill", &args("-f server.py")).unwrap().contains("Kill the specific PID"));
    assert!(refusal("killall", &args("node")).is_some());
}

#[test]
fn install_links_every_tool_and_prepends_the_guard() {
    let temp = crate::test_support::Temp::new();
    let data = temp.dir("data");
    let dir = install(&data).unwrap();
    for tool in TOOLS {
        assert!(dir.join(format!("{tool}{}", env::consts::EXE_SUFFIX)).is_file());
    }
    assert_eq!(install(&data).unwrap(), dir);
    assert!(dir.join("shell-check.txt").is_file());
    let [(path_key, path), (variable, value)] = environment(&dir).unwrap();
    assert_eq!((path_key, variable, value), ("PATH", VARIABLE, dir.clone().into_os_string()));
    assert_eq!(env::split_paths(&path).next().unwrap(), dir);
}
