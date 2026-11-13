use crate::history::{Observed, Usage};
use crate::{Result, engine::worker_statuses, lock, now, read, write};
use serde_json::{Value, json};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::{io::AsRawHandle, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
        JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
    },
};
pub fn command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.creation_flags(0x08000000);
    command
}
pub fn local_json(port: u16, route: &str, body: Option<&Value>) -> Result<Value> {
    let mut curl = command("curl.exe");
    curl.args(["-q", "-sS", "--max-time", "2", "--noproxy", "*"]);
    if let Some(body) = body {
        curl.args(["-H", "Content-Type: application/json", "-d", &body.to_string()]);
    }
    let output = curl.arg(format!("http://127.0.0.1:{port}/api/{route}")).output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let value: Value = serde_json::from_slice(&output.stdout)?;
    if let Some(error) = value["error"].as_str().filter(|error| !error.is_empty()) {
        return Err(error.to_owned().into());
    }
    Ok(value)
}
pub fn local_codex(args: &[String]) -> bool {
    args.len() == 2 && matches!(args[1].as_str(), "--codex" | "--agent" | "--external") && Path::new(&args[0]).file_stem().is_some_and(|name| name.eq_ignore_ascii_case("agent-coordinator"))
}
fn job_result(status: i32) -> Result<()> {
    if status == 0 { Err(std::io::Error::last_os_error().into()) } else { Ok(()) }
}
struct Job(HANDLE);
impl Job {
    fn new() -> Result<Self> {
        unsafe {
            let job = Self(CreateJobObjectW(std::ptr::null(), std::ptr::null()));
            if job.0.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            job_result(SetInformationJobObject(job.0, JobObjectExtendedLimitInformation, &info as *const _ as _, std::mem::size_of_val(&info) as u32))?;
            Ok(job)
        }
    }
    fn stop(&self) -> Result<()> {
        job_result(unsafe { TerminateJobObject(self.0, 1) })
    }
    fn drain(&self) -> Result<()> {
        let mut terminated = false;
        loop {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
            job_result(unsafe {
                QueryInformationJobObject(self.0, JobObjectBasicAccountingInformation, (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(), std::mem::size_of_val(&info) as u32, std::ptr::null_mut())
            })?;
            if info.ActiveProcesses == 0 {
                return Ok(());
            }
            if !terminated {
                self.stop()?;
                terminated = true;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn forward_input(child: &mut Child, input: Vec<u8>) {
    let mut pipe = child.stdin.take().unwrap();
    thread::spawn(move || {
        let _ = pipe.write_all(&input);
    });
}
pub fn supervise(directory: &Path) -> Result<()> {
    let Ok(_guard) = lock(&directory.join("lock")) else {
        return Ok(());
    };
    if directory.join("exit.json").exists() {
        return Ok(());
    }
    let job = match Job::new() {
        Ok(job) => job,
        Err(error) => return write(&directory.join("exit.json"), &json!({"error":error.to_string(),"ended":now()})),
    };
    let run = || -> Result<Value> {
        let request: Value = read(&directory.join("request.json"))?;
        let resumable = request["command"][1] == "--external";
        if !resumable && (directory.join("run.json").exists() || directory.join("launch-intent.json").exists()) {
            if read::<Value>(&directory.join("result.json")).is_ok() {
                return Ok(json!({"code":0,"recovered":true}));
            }
            return Ok(json!({"error":"Supervisor interrupted; its worker job was terminated","interrupted":true}));
        }
        if directory.join("stop").exists() {
            return Ok(json!({"stopped":true}));
        }
        if request["scratch"].is_string() {
            std::fs::create_dir_all(directory.join("scratch"))?;
            std::fs::create_dir_all(directory.join("deliverables"))?;
        }
        if let Err(error) = crate::metrics::project_capture(directory, &request, false) {
            let _ = write(&directory.join("measurement-error.json"), &json!({"at":now(),"error":error.to_string()}));
        }
        let scope: Vec<PathBuf> = serde_json::from_value(request["task"]["writable"].clone())?;
        if !scope.is_empty() && !directory.join("baseline.json").exists() {
            let data = directory.parent().and_then(Path::parent).ok_or("Missing data directory")?;
            write(&directory.join("baseline.json"), &crate::metrics::capture(&scope, data))?;
        }
        if directory.join("stop").exists() {
            return Ok(json!({"stopped":true}));
        }
        let log = OpenOptions::new().create(true).append(true).open(directory.join("log.txt"))?;
        write(&directory.join("launch-intent.json"), &json!({"at":now()}))?;
        let mut child = command(std::env::current_exe()?.to_str().ok_or("Invalid executable path")?)
            .arg("--worker")
            .current_dir(request["task"]["cwd"].as_str().ok_or("Missing cwd")?)
            .env("COORDINATOR_REQUEST", directory.join("request.json"))
            .env("COORDINATOR_RESULT", directory.join("result.json"))
            .env("COORDINATOR_SCRATCH", directory.join("scratch"))
            .stdin(Stdio::piped())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?;
        if let Err(error) = job_result(unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle() as _) }) {
            child.kill()?;
            child.wait()?;
            return Err(error);
        }
        write(&directory.join("run.json"), &json!({"supervisor":std::process::id(),"worker":child.id(),"started":now()}))?;
        forward_input(&mut child, serde_json::to_vec(&request)?);
        let mut stopped = false;
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(json!({"code":status.code(),"stopped":stopped}));
            }
            if !stopped && directory.join("stop").exists() {
                job.stop()?;
                stopped = true;
            }
            thread::sleep(Duration::from_millis(100));
        }
    };
    let mut outcome = run().unwrap_or_else(|error| json!({"error":error.to_string()}));
    job.drain()?;
    outcome["ended"] = json!(now());
    if directory.join("baseline.json").exists()
        && outcome["interrupted"] != true
        && !directory.join("final-snapshot.json").exists()
        && let Err(error) = capture_final(directory)
    {
        eprintln!("Could not record final scope snapshot: {error}");
    }
    if outcome["interrupted"] != true
        && let Ok(request) = read::<Value>(&directory.join("request.json"))
        && let Err(error) = crate::metrics::project_capture(directory, &request, true)
    {
        let _ = write(&directory.join("measurement-error.json"), &json!({"at":now(),"error":error.to_string()}));
    }
    if !directory.join("usage.json").exists()
        && let Some(components) = crate::telemetry::resource_usage(&directory.join("events.jsonl"))
    {
        let _ = write(&directory.join("usage.json"), &Usage { total_tokens: crate::telemetry::total(&components), components: Some(components), source: "codex_completed_turn_events".into() });
    }
    write(&directory.join("exit.json"), &outcome)
}
fn capture_final(directory: &Path) -> Result<()> {
    let request: Value = read(&directory.join("request.json"))?;
    let scope: Vec<PathBuf> = serde_json::from_value(request["task"]["writable"].clone())?;
    let data = directory.parent().and_then(Path::parent).ok_or("Missing data directory")?;
    let snapshot = crate::metrics::capture(&scope, data);
    write(&directory.join("final-snapshot.json"), &snapshot)?;
    let base = Path::new(request["task"]["cwd"].as_str().ok_or("Missing cwd")?);
    write(&directory.join("changes.json"), &crate::metrics::compare(read(&directory.join("baseline.json"))?, snapshot, base, &[]))
}
pub fn worker() -> Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let request: Value = serde_json::from_str(&input)?;
    let selection = request_agent(&request)?;
    let args: Vec<String> = serde_json::from_value(request["command"].clone())?;
    crate::require(local_codex(&args) || (args.iter().any(|arg| arg == "{model}") && args.iter().any(|arg| arg.contains("{effort}"))), "Worker command requires explicit {model} and {effort} placeholders")?;
    let args: Vec<_> = args.iter().map(|arg| arg.replace("{model}", &selection.model).replace("{effort}", selection.effort())).collect();
    let mut child = command(&args[0]).args(&args[1..]).stdin(Stdio::piped()).spawn()?;
    forward_input(&mut child, input.into_bytes());
    crate::require(child.wait()?.success(), "Worker command failed")?;
    Ok(())
}
pub(crate) fn schema(stage: &str) -> Value {
    let text = json!({"type":"string"});
    let list = json!({"type":"array","items":{"type":"string"}});
    let lane = json!({"type":"string","enum":["code","ui","data","docs","audit","cleanup"]});
    let needs = json!({"type":"array","items":{"type":"string","enum":["network","browser","process","gpu"]}});
    let size = json!({"type":"string","enum":["small","medium"]});
    let languages = json!({"type":"array","items":{"type":"string","enum":crate::history::LANGUAGES}});
    let functions = json!({"type":"array","items":{"type":"string","enum":crate::history::FUNCTIONS}});
    let artifact = json!({"type":"string","enum":["none","candidate","report"]});
    let object = |properties: Value| {
        let required: Vec<String> = properties.as_object().unwrap().keys().cloned().collect();
        json!({"type":"object","additionalProperties":false,"properties":properties,"required":required})
    };
    let dependency = object(json!({"target":{"anyOf":[{"type":"string"},{"type":"integer"}]},"reason":{"type":"string","enum":crate::engine::DEPENDENCY_REASONS}}));
    let dependencies = json!({"type":"array","items":dependency});
    let child = object(
        json!({"title":text,"outcome":text,"acceptance":list,"lane":lane,"needs":needs,"languages":languages,"functions":functions,"size":size,"artifact":artifact,"dependencies":dependencies,"after":{"type":"array","items":{"type":"integer"}}}),
    );
    let mut properties = json!({"status":{"type":"string","enum":worker_statuses(stage)},"summary":text,"checks":list,"findings":list});
    if stage != "replan" {
        properties["criteria"] = json!({"type":"array","items":{"type":"string","enum":["passed","failed","blocked"]}});
    }
    match stage {
        "review" => {}
        "replan" => {
            properties["plan"] = object(
                json!({"title":text,"outcome":text,"acceptance":list,"lane":lane,"needs":needs,"languages":languages,"functions":functions,"size":size,"dependencies":{"type":"array","items":object(json!({"target":text,"reason":{"type":"string","enum":crate::engine::DEPENDENCY_REASONS}}))},"children":{"type":"array","items":child}}),
            )
        }
        _ => properties["blocker"] = json!({"type":"string","enum":["none","capability","dependency","definition","environment","external"]}),
    }
    object(properties)
}
fn limit(request: &Value, key: &str) -> u64 {
    request["limits"][key].as_u64().or_else(|| crate::history::Limits::default().value()[key].as_u64()).unwrap_or_default()
}
fn rules(request: &Value) -> Result<String> {
    let protocol = request["workerProtocol"]["text"].as_str().ok_or("Workspace worker protocol is required")?;
    crate::require((1..=200).contains(&protocol.split_whitespace().count()), "Workspace worker protocol must contain 1..200 words")?;
    Ok(format!(
        "{protocol}\nKeep summary under {} words and at most {} findings of {} words.",
        limit(request, "summaryWords"),
        limit(request, "findings"),
        limit(request, "findingWords")
    ))
}
pub fn external() -> Result<()> {
    let request: Value = read(Path::new(&std::env::var("COORDINATOR_REQUEST")?))?;
    request_agent(&request)?;
    let result = PathBuf::from(std::env::var("COORDINATOR_RESULT")?);
    let directory = result.parent().unwrap();
    let stage = request["stage"].as_str().unwrap_or("implementation");
    let schema_path = directory.join("schema.json");
    write(&schema_path, &schema(stage))?;
    let prompt = format!("{}\n{}\nWhen finished, write one JSON object matching the schema at {} to {}.\n{request}", action(&request, stage), rules(&request)?, schema_path.display(), result.display());
    std::fs::write(directory.join("prompt.md"), prompt)?;
    while read::<Value>(&result).is_err() {
        thread::sleep(Duration::from_secs(1));
    }
    Ok(())
}
fn action(request: &Value, stage: &str) -> String {
    if stage == "replan" {
        return format!(
            "This task is blocked or failed review repeatedly. Inspect task.cwd read-only and recentAttempts, then return a plan that can succeed with the listed capabilities. Prefer splitting into small isolated children: one concern and one lane each, at most {} runnable criteria, title under {} words, under {} words. Order siblings with after indices. Dependencies require target/reason records: uses_artifact, uses_api, uses_schema or uses_data; use after, or nothing otherwise. Child dependency targets may be sibling indices or existing task IDs. Preserve existing dependencies on rewrites. With no children, the plan replaces the task definition. Never put task ids, hashes or coordination instructions in text. Return held only when a decision only the user can make is required; name it in one finding.",
            limit(request, "criteria"),
            limit(request, "titleWords"),
            limit(request, "words")
        );
    }
    match stage {
        "review" => {
            "Independently verify the implementation in task.cwd against each acceptance criterion, read-only. Run the checks yourself; do not trust the implementation summary. Return criteria with one outcome per criterion in order. accepted only if every criterion passed. rejected if any failed; each finding states the failing behavior for the whole class of inputs. blocked only if a required check cannot run in the available environment."
        }
        _ if request["readOnly"] == true => {
            "Perform the read-only audit of task.cwd against the acceptance criteria. Write the report to the absolute path in task.artifact. Return criteria with one outcome per criterion in order; completed only if every criterion passed."
        }
        _ => {
            "Implement the task outcome in task.cwd and run checks proving each acceptance criterion. If feedback lists failed criteria, fix the cause for the whole class of inputs, not only the cited example. Use scratch for disposable files. If task.artifact is set, write that exact absolute path. Return criteria with one outcome per criterion in order; completed only if every criterion passed. Otherwise return blocked with a blocker kind and one finding."
        }
    }
    .into()
}
pub(crate) fn request_agent(request: &Value) -> Result<crate::agents::Agent> {
    let agent: crate::agents::Agent = serde_json::from_value(request.get("agent").cloned().ok_or("Explicit worker agent required")?)?;
    let recorded: Option<crate::agents::Family> = serde_json::from_value(request["family"].clone()).unwrap_or(None);
    crate::require(agent.allowed(crate::agents::launcher().or(recorded)), "Worker agent is not allowed for the family that started this coordinator")?;
    crate::require(!agent.retired, "Worker agent is retired")?;
    Ok(agent)
}
pub(crate) fn prompt(request: &Value, stage: &str) -> Result<String> {
    Ok(format!("{}
{}", action(request, stage), rules(request)?))
}
pub fn codex() -> Result<()> {
    let request: Value = read(Path::new(&std::env::var("COORDINATOR_REQUEST")?))?;
    let agent = request_agent(&request)?;
    match agent.harness {
        crate::agents::Harness::ClaudeCode => return crate::adapters::claude(&request, &agent),
        crate::agents::Harness::Codex => {}
    }
    let result = std::env::var("COORDINATOR_RESULT")?;
    let directory = Path::new(&result).parent().unwrap();
    let stage = request["stage"].as_str().unwrap_or("implementation");
    let schema_path = directory.join("schema.json");
    write(&schema_path, &schema(stage))?;
    let prompt = format!("{}\n{}\n{request}", action(&request, stage), rules(&request)?);
    let program = std::env::var("CODEX_BIN").unwrap_or_else(|_| "codex".into());
    let mut cli = command(&program);
    let profile = std::env::var_os("USERPROFILE").map(PathBuf::from).filter(|path| path.is_absolute());
    cli.envs(profile.filter(|_| std::env::var_os("CODEX_HOME").is_none_or(|home| home.is_empty())).map(|path| ("CODEX_HOME", path.join(".codex"))));
    cli.env_remove("CODEX_WINDOWS_REGISTERED_CORE").arg("exec");
    let model = agent.model.as_str();
    let effort = agent.effort();
    cli.args(["--model", model, "-c", &format!("model_reasoning_effort=\"{effort}\"")]);
    let read_only = request["readOnly"] == true;
    if let Some(scratch) = request["scratch"].as_str() && read_only {
        cli.current_dir(scratch).args(["--cd", scratch]);
    }
    let events = std::fs::File::create(directory.join("events.jsonl"))?;
    let mut child = cli
        .args(["--skip-git-repo-check", "--ephemeral", "--json", "--color", "never", "--dangerously-bypass-approvals-and-sandbox", "--disable", "memories"])
        .arg("--output-schema")
        .arg(schema_path)
        .arg("--output-last-message")
        .arg(&result)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(events)
        .stderr(Stdio::piped())
        .spawn()?;
    let evidence = Observed {
        worker: Path::new(&program).file_name().unwrap_or_default().to_string_lossy().into(),
        cli_version: None,
        model: None,
        effort: None,
        forwarded_model: Some(model.to_owned()),
        forwarded_effort: Some(effort.to_owned()),
        source: "codex_launch".into(),
    };
    let stderr = child.stderr.take().unwrap();
    let path = directory.join("launch-settings.json");
    let reader = thread::spawn(move || record_codex_launch(stderr, &path, evidence));
    let input = child.stdin.take().unwrap().write_all(prompt.as_bytes());
    let status = child.wait()?;
    let tokens = reader.join().map_err(|_| "Codex output reader interrupted")??;
    input?;
    crate::evidence::capture_trace(directory)?;
    let components = crate::telemetry::resource_usage(&directory.join("events.jsonl"));
    if tokens.is_some() || components.is_some() {
        write(&directory.join("usage.json"), &Usage { total_tokens: tokens.or_else(|| components.as_ref().and_then(crate::telemetry::total)), components, source: "codex_token_footer".into() })?;
    }
    crate::require(status.success(), "Codex worker failed; inspect attempt log")?;
    Ok(())
}
fn record_codex_launch(stderr: impl Read, path: &Path, mut evidence: Observed) -> Result<Option<u64>> {
    let mut preamble = true;
    let mut line = Vec::new();
    let mut stream = BufReader::new(stderr);
    let mut output = std::io::stderr();
    let mut token_label = false;
    let mut tokens = None;
    while stream.read_until(b'\n', &mut line)? != 0 {
        output.write_all(&line)?;
        let text = String::from_utf8_lossy(&line);
        let text = text.trim();
        if preamble {
            let version = text.strip_prefix("OpenAI Codex v").filter(|s| s.starts_with(|c: char| c.is_ascii_digit()) && s.chars().all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c)));
            evidence.cli_version = version.map(str::to_owned).or(evidence.cli_version);
            if evidence.cli_version.is_some() {
                let model = text.strip_prefix("model: ").filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:".contains(c)));
                let effort = text.strip_prefix("reasoning effort: ").filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphabetic()));
                evidence.model = model.map(str::to_owned).or(evidence.model);
                evidence.effort = effort.map(str::to_owned).or(evidence.effort);
                if model.is_some() || effort.is_some() {
                    evidence.source = "codex_startup_banner".into();
                }
            }
            if matches!(text, "user" | "codex" | "assistant") {
                write(path, &evidence)?;
                preamble = false;
            }
        } else if !text.is_empty() {
            tokens = if token_label { text.replace(',', "").parse().ok() } else { None };
            token_label = text == "tokens used";
        }
        line.clear();
    }
    if preamble {
        write(path, &evidence)?;
    }
    Ok(tokens.filter(|_| evidence.cli_version.is_some()))
}
