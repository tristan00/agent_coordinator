use crate::process::{command, local_json};
use crate::{Result, lock, read, require, write};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
pub fn frontend_digest() -> String {
    let mut hash = Sha256::new();
    for asset in [include_bytes!("../web/index.html").as_slice(), include_bytes!("../web/app.js").as_slice(), include_bytes!("../web/style.css").as_slice()] {
        hash.update((asset.len() as u64).to_le_bytes());
        hash.update(asset);
    }
    format!("sha256:{:x}", hash.finalize())
}
pub fn event(directory: &Path) -> Value {
    read(&directory.join("redeployment.json")).unwrap_or(Value::Null)
}
pub fn receipt(directory: &Path, id: &str) -> Result<Value> {
    require(!id.is_empty() && id.chars().all(|c| c.is_ascii_digit() || c == '-'), "Invalid request ID")?;
    read(&directory.join("redeploy-requests").join(format!("{id}.result.json")))
}
pub fn enqueue(input: Value, directory: &Path, port: u16) -> Result<Value> {
    validate(&input)?;
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = format!("{}-{}-{}", crate::now(), std::process::id(), SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let path = directory.join("redeploy-requests").join(format!("{id}.json"));
    write(&path, &input)?;
    write(&path.with_extension("result.json"), &json!({"status":"queued","id":id}))?;
    crate::web::server_command(std::env::current_exe()?.to_str().ok_or("Invalid executable")?, directory, port)?.arg("--redeploy-request").arg(path).spawn()?;
    Ok(json!({"id":id,"status":"queued","receipt":format!("/api/deployment/requests/{id}")}))
}
fn validate(input: &Value) -> Result<()> {
    require(input["recover"] == true || (input["source"].is_string() ^ input["candidate"].is_string()), "Supply source directory, candidate executable, or recover:true")
}
pub fn request(path: &Path) -> Result<()> {
    let input = read(path)?;
    let result = execute(input).unwrap_or_else(|e| json!({"status":"failed","error":e.to_string(),"recovery":"Retry --redeploy recover using the same COORDINATOR_DATA and PORT"}));
    write(&path.with_extension("result.json"), &result)?;
    require(result["status"] == "verified_live", result.to_string())
}
pub fn cli(args: &[String]) -> Result<()> {
    let value = args.first().ok_or("Usage: --redeploy SOURCE_DIRECTORY|CANDIDATE_EXE|recover")?;
    let input = if value == "recover" {
        json!({"recover":true})
    } else if Path::new(value).is_dir() {
        json!({"source":value})
    } else {
        json!({"candidate":value})
    };
    let result = execute(input)
        .unwrap_or_else(|e| json!({"status":"failed","error":e.to_string(),"recovery":"Retry --redeploy recover with the same COORDINATOR_DATA and PORT; inspect redeployment.json and deployment-server.log"}));
    println!("{result}");
    require(result["status"] == "verified_live", "Deployment failed; see JSON receipt")
}
fn identity(path: &Path) -> Result<Value> {
    use std::io::Read;
    use std::process::Stdio;
    let digest = crate::evidence::digest(path)?;
    let mut child = command(path.to_str().ok_or("Invalid candidate path")?).arg("--identity").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    let mut pipe = child.stdout.take().unwrap();
    let output = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).map(|_| bytes)
    });
    let started = Instant::now();
    let timeout = readiness_timeout()?;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            child.kill()?;
            child.wait()?;
            return Err("Candidate identity timed out; inspect executable or adjust COORDINATOR_READY_TIMEOUT_MS".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    require(status.success(), "Candidate identity command failed")?;
    let mut identity: Value = serde_json::from_slice(&output.join().map_err(|_| "Identity reader failed")??)?;
    require(identity["protocol"] == 1 && identity["digest"] == digest && identity["frontend"].is_string(), "Candidate does not support verified redeployment")?;
    identity["executable"] = json!(path.canonicalize()?);
    Ok(identity)
}
fn readiness_timeout() -> Result<Duration> {
    Ok(Duration::from_millis(std::env::var("COORDINATOR_READY_TIMEOUT_MS").ok().map(|v| v.parse()).transpose()?.unwrap_or(20000)))
}
fn shell_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    if let Some(unc) = path.strip_prefix("\\\\?\\UNC\\") { format!("\\\\{unc}") } else { path.strip_prefix("\\\\?\\").unwrap_or(&path).to_owned() }
}
fn port(directory: &Path) -> Result<u16> {
    if let Ok(value) = std::env::var("PORT") {
        return Ok(value.parse()?);
    }
    Ok(read(&directory.join("port.json")).unwrap_or(4242))
}
pub fn authorize_handoff(directory: &Path, input: &Value) -> Result<()> {
    let pending = event(directory);
    require(pending["status"] == "handoff" && pending["id"] == input["id"] && pending["from"]["digest"] == input["expected"], "No matching durable handoff")?;
    require(lock(&directory.join("redeploy.lock")).is_err(), "Deployment owner is no longer running")
}
fn live(directory: &Path, port: u16) -> Result<Option<Value>> {
    match local_json(port, "deployment", None) {
        Ok(state) => {
            let actual = Path::new(state["dataDirectory"].as_str().ok_or("Server did not identify its data directory")?).canonicalize()?;
            require(actual == directory.canonicalize()?, "Port belongs to a different board")?;
            Ok(Some(state["live"].clone()))
        }
        Err(error) => {
            let guard = lock(&directory.join("server.lock")).map_err(|_| format!("Board holds server.lock but API is unavailable: {error}"))?;
            drop(guard);
            require(std::net::TcpStream::connect(("127.0.0.1", port)).is_err(), "Port is occupied by an unrecognized server")?;
            Ok(None)
        }
    }
}
fn wait_release(directory: &Path, timeout: Duration) -> Result<()> {
    let started = Instant::now();
    loop {
        if let Ok(guard) = lock(&directory.join("server.lock")) {
            drop(guard);
            return Ok(());
        }
        require(started.elapsed() < timeout, "Server has not released its lock; retry recover")?;
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn launch(directory: &Path, port: u16, target: &Value, timeout: Duration) -> Result<Value> {
    let path = target["executable"].as_str().ok_or("Missing executable")?;
    require(crate::evidence::digest(Path::new(path))? == target["digest"], "Executable changed before launch")?;
    let mut child = crate::web::server_command(path, directory, port)?.spawn()?;
    let start = Instant::now();
    let outcome = loop {
        if let Some(status) = child.try_wait()? {
            break Err(format!("Candidate exited: {status}").into());
        }
        if let Ok(Some(current)) = live(directory, port)
            && current["pid"] == child.id()
            && current["digest"] == target["digest"]
            && current["frontend"] == target["frontend"]
        {
            break Ok(current);
        }
        if start.elapsed() >= timeout {
            break Err("Readiness timeout; inspect deployment-server.log".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if outcome.is_err() && child.try_wait()?.is_none() {
        child.kill()?;
        child.wait()?;
    }
    outcome
}
fn finish(directory: &Path, port: u16, mut pending: Value, timeout: Duration) -> Result<Value> {
    let started = Instant::now();
    let current = live(directory, port)?;
    if let Some(current) = &current {
        if current["digest"] == pending["target"]["digest"] {
            pending["verified"] = current.clone();
        } else {
            require(current["digest"] == pending["from"]["digest"], "Unexpected server identity; refusing handoff")?;
            local_json(port, "deployment/quiesce", Some(&json!({"expected":current["digest"],"id":pending["id"]})))?;
            wait_release(directory, timeout)?;
        }
    }
    let outcome = if pending["verified"].is_object() { Ok(pending["verified"].clone()) } else { launch(directory, port, &pending["target"], timeout) };
    match outcome {
        Ok(verified) => {
            pending["status"] = json!("verified_live");
            pending["verified"] = verified;
        }
        Err(error) => {
            pending["status"] = json!("failed");
            pending["error"] = json!(error.to_string());
            if pending["from"].is_object() {
                pending["rollback"] = match launch(directory, port, &pending["from"], timeout) {
                    Ok(verified) => verified,
                    Err(error) => json!({"error":error.to_string()}),
                };
            }
        }
    }
    pending["handoffMs"] = json!(started.elapsed().as_millis());
    write(&directory.join("redeployment.json"), &pending)?;
    write(&directory.join("redeployments").join(format!("{}.json", pending["id"].as_str().unwrap_or("recovery"))), &pending)?;
    Ok(pending)
}
fn execute(input: Value) -> Result<Value> {
    validate(&input)?;
    let directory = crate::web::data_directory()?;
    let _guard = lock(&directory.join("redeploy.lock")).map_err(|_| "Another deployment owns redeploy.lock; retry after its receipt completes")?;
    let _legacy = lock(&directory.join("deployment.lock"))?;
    let _helper = lock(&directory.join("deployment-helper.lock"))?;
    let port = port(&directory)?;
    let timeout = readiness_timeout()?;
    let pending = event(&directory);
    if pending["status"] == "handoff" {
        let recovered = finish(&directory, port, pending, timeout)?;
        if input["recover"] == true || recovered["status"] != "verified_live" {
            return Ok(recovered);
        }
    }
    if input["recover"] == true {
        let previous = event(&directory);
        if let Some(current) = live(&directory, port)? {
            require(current["digest"] == previous["verified"]["digest"] || current["digest"] == previous["rollback"]["digest"], "Live server does not match recovery receipt")?;
            return Ok(json!({"status":"verified_live","verified":current,"recovered":true}));
        }
        let target = if previous["rollback"]["digest"].is_string() { previous["rollback"].clone() } else { previous["verified"].clone() };
        require(target.is_object(), "No retained deployment; supply a prepared candidate or source directory")?;
        let verified = launch(&directory, port, &target, timeout)?;
        return Ok(json!({"status":"verified_live","verified":verified,"recovered":true}));
    }
    let started = Instant::now();
    let mut build = json!({"mode":"prepared"});
    let candidate = if let Some(source) = input["source"].as_str() {
        let source = Path::new(source).canonicalize()?;
        let output = command("pwsh.exe")
            .args(["-NoProfile", "-File"])
            .arg(shell_path(&source.join("scripts/redeploy-build.ps1")))
            .arg("-Source")
            .arg(shell_path(&source))
            .arg("-Cache")
            .arg(shell_path(&directory.join("build")))
            .output()?;
        require(output.status.success(), format!("Build failed: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)))?;
        build = serde_json::from_slice(&output.stdout)?;
        PathBuf::from(build["candidate"].as_str().ok_or("Build omitted candidate")?)
    } else {
        PathBuf::from(input["candidate"].as_str().ok_or("Missing candidate")?).canonicalize()?
    };
    let checked = identity(&candidate)?;
    if build["mode"] == "source" {
        require(checked["frontendSource"] == build["frontendSource"] && checked["source"] == build["source"] && checked["frontend"] == build["frontend"], "Build contains stale source or frontend")?;
    }
    let staged = directory.join("deployed").join(checked["digest"].as_str().unwrap().trim_start_matches("sha256:")).join("agent-coordinator.exe");
    fs::create_dir_all(staged.parent().unwrap())?;
    if crate::evidence::digest(&staged).ok().as_ref() != checked["digest"].as_str().map(str::to_owned).as_ref() {
        let temporary = staged.with_extension(format!("{}.tmp", std::process::id()));
        fs::copy(&candidate, &temporary)?;
        require(crate::evidence::digest(&temporary)? == checked["digest"], "Candidate changed while staging")?;
        fs::rename(temporary, &staged)?;
    }
    let target = identity(&staged)?;
    require(target["digest"] == checked["digest"], "Staged executable digest mismatch")?;
    let from = live(&directory, port)?;
    let mut pending = json!({"id":format!("{}-{}",crate::now(),std::process::id()),"status":"handoff","from":from,"target":target,"build":build,"buildMs":started.elapsed().as_millis()});
    if pending["from"].is_null() {
        let previous = event(&directory);
        pending["from"] = if previous["rollback"]["digest"].is_string() { previous["rollback"].clone() } else { previous["verified"].clone() };
    }
    write(&directory.join("redeployment.json"), &pending)?;
    finish(&directory, port, pending, timeout)
}
