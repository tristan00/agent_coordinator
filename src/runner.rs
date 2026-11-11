use crate::{engine, model::*, store::*};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{io::AsyncWriteExt, process::Command};

#[derive(Clone)]
pub struct Runner {
    pub store: Arc<Store>,
    pub data_dir: PathBuf,
    pub stopping: Arc<AtomicBool>,
    pub wake: Arc<tokio::sync::Notify>,
    pub health: Arc<Mutex<(String, Option<String>)>>,
    pub draining: Arc<AtomicBool>,
}
impl Runner {
    pub fn new(store: Arc<Store>, data_dir: PathBuf) -> Self {
        Self {
            store,
            data_dir,
            stopping: Arc::new(AtomicBool::new(false)),
            wake: Arc::new(tokio::sync::Notify::new()),
            health: Arc::new(Mutex::new((now(), None))),
            draining: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn start_scheduler(&self) {
        crate::callbacks::start_dispatcher(self.store.clone(), self.stopping.clone());
        let runner = self.clone();
        tokio::spawn(async move {
            while !runner.stopping.load(Ordering::Relaxed) {
                let result = runner.store.mutate(|s| {
                    engine::reconcile(s)?;
                    if runner.draining.load(Ordering::Relaxed) {
                        Ok(None)
                    } else {
                        crate::workers::dispatch(s)?;
                        engine::claim(s, None)
                    }
                });
                if let Ok(mut health) = runner.health.lock() {
                    *health = (now(), result.as_ref().err().map(ToString::to_string));
                }
                match result {
                    Ok(Some((attempt, agent))) => {
                        runner.launch(attempt, agent);
                        continue;
                    }
                    Err(e) => eprintln!("Coordinator tick failed: {e}"),
                    _ => {}
                }
                tokio::select! { _ = tokio::time::sleep(Duration::from_millis(500)) => {}, _ = runner.wake.notified() => {} }
            }
        });
    }
    pub fn launch(&self, attempt: Attempt, agent: Agent) {
        let runner = self.clone();
        tokio::spawn(async move {
            let mut result = runner.execute(&attempt, &agent).await;
            loop {
                let saved = match &result {
                    Ok((output, log)) => runner
                        .store
                        .mutate(|s| engine::finish(s, &attempt.id, output.clone(), log.clone())),
                    Err(error) => runner
                        .store
                        .mutate(|s| engine::fail(s, &attempt.id, &error.1, "")),
                };
                match saved {
                    Ok(()) => break,
                    Err(error) if error.0 >= 500 => {
                        // The process already finished. Retain its outcome and
                        // retry persistence instead of abandoning a live claim.
                        eprintln!("Could not save worker outcome; retrying: {error}");
                        if runner.stopping.load(Ordering::Relaxed) {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    Err(error) if result.is_ok() => {
                        result = Err(bad(format!("Result rejected: {error}")));
                    }
                    Err(error) => {
                        eprintln!("Attempt no longer accepts its outcome: {error}");
                        break;
                    }
                }
            }
            runner.wake.notify_one();
        });
    }
    pub fn live(&self, attempt: &str) -> bool {
        !self.stopping.load(Ordering::Relaxed)
            && self.store.read().map_or(true, |s| {
                s.attempts
                    .iter()
                    .any(|a| a.id == attempt && a.status == "running")
            })
    }
    async fn execute(&self, attempt: &Attempt, agent: &Agent) -> AppResult<(AgentResult, String)> {
        let dir = self.data_dir.join("runs").join(&attempt.id);
        tokio::fs::create_dir_all(&dir).await?;
        let input = serde_json::to_string_pretty(&attempt.input)?;
        tokio::fs::write(dir.join("input.json"), &input).await?;
        let prompt = format!(
            "Execute the assigned stage and return JSON matching result_contract.\n\n{input}"
        );
        tokio::fs::write(dir.join("prompt.md"), &prompt).await?;
        let timeout = [
            agent.timeout_seconds,
            attempt.input["task"]["timeout_seconds"]
                .as_u64()
                .unwrap_or(0),
        ]
        .into_iter()
        .filter(|seconds| *seconds > 0)
        .min();
        let operation = async {
            match agent.adapter.as_str() {
                "demo" => {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    Ok((
                        demo_result(&attempt.input),
                        "Simulated execution. No model was called or workspace changed.".into(),
                    ))
                }
                "cli" => self.cli(attempt, agent, &dir, &input, &prompt).await,
                "ollama" => self.ollama(agent, &prompt).await,
                _ => Err(bad("Unknown adapter")),
            }
        };
        tokio::pin!(operation);
        let timeout_future = async {
            match timeout.and_then(|seconds| {
                tokio::time::Instant::now().checked_add(Duration::from_secs(seconds))
            }) {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::pin!(timeout_future);
        let result = loop {
            tokio::select! {
                output=&mut operation=>break output,
                _=&mut timeout_future=>break Err(bad(format!("Execution exceeded {} seconds", timeout.unwrap()))),
                _=tokio::time::sleep(Duration::from_millis(200))=> {if !self.live(&attempt.id){break Err(bad("Execution cancelled"));}}
            }
        };
        if let Ok((output, log)) = &result {
            tokio::fs::write(dir.join("result.json"), serde_json::to_vec_pretty(output)?).await?;
            tokio::fs::write(dir.join("execution.log"), log).await?;
        }
        result
    }
    async fn cli(
        &self,
        attempt: &Attempt,
        agent: &Agent,
        dir: &std::path::Path,
        input: &str,
        prompt: &str,
    ) -> AppResult<(AgentResult, String)> {
        let workspace = attempt.input["task"]["workspace"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| dir.to_path_buf());
        let output_path = dir.join("agent-result.json");
        let args: Vec<String> = agent
            .args
            .iter()
            .map(|a| {
                a.replace("{input}", &dir.join("input.json").to_string_lossy())
                    .replace("{output}", &output_path.to_string_lossy())
                    .replace("{workspace}", &workspace.to_string_lossy())
                    .replace("{prompt_file}", &dir.join("prompt.md").to_string_lossy())
                    .replace("{prompt}", prompt)
            })
            .collect();
        let mut command = Command::new(&agent.command);
        command.args(args);
        for name in &agent.unset_env {
            command.env_remove(name);
        }
        command
            .current_dir(&workspace)
            .stdin(Stdio::piped())
            // Write directly to files: verbose agents cannot fill a pipe or
            // exhaust an in-memory log buffer, and cancellation preserves logs.
            .stdout(Stdio::from(std::fs::File::create(dir.join("stdout.log"))?))
            .stderr(Stdio::from(std::fs::File::create(dir.join("stderr.log"))?))
            .kill_on_drop(true);
        command
            .env("RELAY_INPUT", dir.join("input.json"))
            .env("RELAY_OUTPUT", &output_path)
            .env("RELAY_ATTEMPT_ID", &attempt.id);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|e| bad(format!("Could not start {}: {e}", agent.command)))?;
        let guard = ProcessGuard::new(child.id())?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| bad("Could not open stdin"))?;
        let input_bytes = input.as_bytes().to_vec();
        let writer = tokio::spawn(async move {
            let _ = stdin.write_all(&input_bytes).await;
            let _ = stdin.shutdown().await;
        });
        let status = child.wait().await?;
        drop(guard);
        writer.abort();
        let stderr_path = dir.join("stderr.log");
        let stderr = tokio::task::spawn_blocking(move || log_tail(&stderr_path))
            .await
            .map_err(|e| bad(e.to_string()))??;
        if !status.success() {
            return Err(bad(format!("Agent exited with {status}. {stderr}")));
        }
        let result_path = if output_path.exists() {
            output_path
        } else {
            dir.join("stdout.log")
        };
        let result = tokio::task::spawn_blocking(move || parse_result_file(&result_path))
            .await
            .map_err(|e| bad(e.to_string()))??;
        Ok((result, stderr))
    }
    async fn ollama(&self, agent: &Agent, prompt: &str) -> AppResult<(AgentResult, String)> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|e| bad(e.to_string()))?;
        let schema = json!({"type":"object","properties":{"estimate":{"type":"object","properties":{"points":{"type":"integer","enum":[1,2,3,5,8,13]},"confidence":{"type":"string","enum":["low","medium","high"]},"reason":{"type":"string"}},"required":["points","confidence","reason"]},"outcome":{"type":"string","enum":["complete","send_back","blocked","needs_input","failed"]},"summary":{"type":"string"},"reason":{"type":"string"},"send_back_to":{"type":["string","null"]},"artifacts":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"},"content":{"type":"string"}},"required":["name","content"]}}},"required":["outcome","summary","artifacts"]});
        let response=client.post(format!("{}/api/chat",agent.endpoint.trim_end_matches('/'))).json(&json!({"model":agent.model,"stream":false,"format":schema,"messages":[{"role":"system","content":"You produce text artifacts only. You cannot inspect or modify the workspace or run checks. If the task needs those capabilities, return blocked and explain that a CLI agent is required."},{"role":"user","content":prompt}]})).send().await.map_err(|e|bad(format!("Ollama connection failed: {e}")))?;
        if !response.status().is_success() {
            return Err(bad(format!("Ollama returned HTTP {}", response.status())));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|e| bad(format!("Invalid Ollama response: {e}")))?;
        let mut result = parse_result(
            body["message"]["content"]
                .as_str()
                .ok_or_else(|| bad("Ollama response has no message content"))?,
        )?;
        result.usage = Usage {
            input_tokens: body["prompt_eval_count"].as_u64(),
            output_tokens: body["eval_count"].as_u64(),
            cost_usd: None,
            source: "reported".into(),
        };
        result.model = body["model"].as_str().map(str::to_string);
        Ok((
            result,
            "Local Ollama response. Text-only adapter; no workspace tools.".into(),
        ))
    }
}

// This is only the amount displayed/stored in the task snapshot. The complete
// stream stays on disk, and crossing this size never fails an execution.
fn log_tail(path: &std::path::Path) -> AppResult<String> {
    const DISPLAY_BYTES: u64 = 64 * 1024;
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(DISPLAY_BYTES)))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let tail = String::from_utf8_lossy(&bytes);
    Ok(if size > DISPLAY_BYTES {
        format!(
            "[Showing the final {DISPLAY_BYTES} bytes; complete log: {}]\n{tail}",
            path.display()
        )
    } else {
        tail.into_owned()
    })
}

fn parse_result_file(path: &std::path::Path) -> AppResult<AgentResult> {
    let file = std::fs::File::open(path)?;
    if let Ok(result) = serde_json::from_reader::<_, AgentResult>(BufReader::new(file)) {
        engine::validate_result(&result)?;
        return Ok(result);
    }
    // Locate JSON starts using fixed-size reads, including pretty-printed or
    // fenced output. Parse candidates from the file itself rather than loading
    // the entire log, or an arbitrarily long diagnostic line, into memory.
    let mut scanner = BufReader::new(std::fs::File::open(path)?);
    let mut offset = 0u64;
    // Prefix states also recognize the existing inline ```json { ... } ```
    // format, without buffering an arbitrarily long line of diagnostic text.
    let mut prefix = 0u8;
    let mut result = None;
    loop {
        let buffer = scanner.fill_buf()?;
        if buffer.is_empty() {
            break;
        }
        let mut starts = Vec::new();
        for (index, byte) in buffer.iter().enumerate() {
            if *byte == b'\n' {
                prefix = 0;
            } else {
                match prefix {
                    0 | 3 | 7 if byte.is_ascii_whitespace() => {}
                    0 | 3 | 7 if *byte == b'{' => {
                        starts.push(offset + index as u64);
                        prefix = 8;
                    }
                    0 if *byte == b'`' => prefix = 1,
                    1 | 2 if *byte == b'`' => prefix += 1,
                    3 if *byte == b'j' => prefix = 4,
                    4 if *byte == b's' => prefix = 5,
                    5 if *byte == b'o' => prefix = 6,
                    6 if *byte == b'n' => prefix = 7,
                    _ => prefix = 8,
                }
            }
        }
        let consumed = buffer.len();
        scanner.consume(consumed);
        offset += consumed as u64;
        for start in starts {
            let mut candidate = std::fs::File::open(path)?;
            candidate.seek(SeekFrom::Start(start))?;
            let mut parser = serde_json::Deserializer::from_reader(BufReader::new(candidate))
                .into_iter::<AgentResult>();
            if let Some(Ok(value)) = parser.next() {
                if result_line_ends(path, start + parser.byte_offset() as u64)? {
                    result = Some(value);
                }
            }
        }
    }
    let result = result.ok_or_else(|| {
        bad("Agent must return a valid result JSON object on stdout or write it to RELAY_OUTPUT")
    })?;
    engine::validate_result(&result)?;
    Ok(result)
}

fn result_line_ends(path: &std::path::Path, offset: u64) -> AppResult<bool> {
    let mut reader = BufReader::new(std::fs::File::open(path)?);
    reader.seek(SeekFrom::Start(offset))?;
    let mut ticks = 0;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(ticks == 0 || ticks == 3);
        }
        for byte in buffer {
            if *byte == b'\n' {
                return Ok(ticks == 0 || ticks == 3);
            }
            if *byte == b'`' && ticks < 3 {
                ticks += 1;
            } else if !byte.is_ascii_whitespace() || (ticks != 0 && ticks != 3) {
                return Ok(false);
            }
        }
        let consumed = buffer.len();
        reader.consume(consumed);
    }
}

#[cfg(test)]
mod file_result_tests {
    use super::*;

    #[test]
    fn disk_result_parser_preserves_fences_and_rejects_malformed_json_suffixes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("stdout.log");
        let result = json!({"outcome":"complete","summary":"Checked", "artifacts":[{"name":"result","content":"deliverable"}]}).to_string();
        for raw in [
            result.clone(),
            format!("```json {result} ```"),
            format!("diagnostics\n{result}\nmore diagnostics"),
        ] {
            std::fs::write(&file, raw).unwrap();
            assert!(parse_result_file(&file).is_ok());
        }
        std::fs::write(&file, format!("{result} malformed suffix")).unwrap();
        assert!(parse_result_file(&file).is_err());
    }
}

struct ProcessGuard {
    #[cfg(windows)]
    job: isize,
    #[cfg(unix)]
    pid: Option<u32>,
}
impl ProcessGuard {
    fn new(pid: Option<u32>) -> AppResult<Self> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::{
                Foundation::{CloseHandle, HANDLE},
                System::{JobObjects::*, Threading::*},
            };
            let pid = pid.ok_or_else(|| bad("Agent process has no PID"))?;
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return Err(bad(format!(
                        "Could not create process job: {}",
                        std::io::Error::last_os_error()
                    )));
                }
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32,
                ) == 0
                {
                    CloseHandle(job);
                    return Err(bad("Could not configure process cleanup"));
                }
                let process: HANDLE =
                    OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false as i32, pid);
                if process.is_null() {
                    CloseHandle(job);
                    return Err(bad("Could not supervise agent process"));
                }
                let assigned = AssignProcessToJobObject(job, process);
                CloseHandle(process);
                if assigned == 0 {
                    let error = std::io::Error::last_os_error();
                    CloseHandle(job);
                    return Err(bad(format!(
                        "Could not attach agent process to cleanup job: {error}"
                    )));
                }
                Ok(Self { job: job as isize })
            }
        }
        #[cfg(unix)]
        {
            Ok(Self { pid })
        }
    }
}
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(
                self.job as windows_sys::Win32::Foundation::HANDLE,
            );
        }
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}
pub fn parse_result(raw: &str) -> AppResult<AgentResult> {
    let cleaned = raw
        .trim()
        .strip_prefix("```json")
        .or_else(|| raw.trim().strip_prefix("```"))
        .unwrap_or(raw.trim())
        .trim()
        .trim_end_matches("```")
        .trim();
    let result=serde_json::from_str::<AgentResult>(cleaned).or_else(|_|raw.lines().rev().find_map(|line|serde_json::from_str::<AgentResult>(line).ok()).ok_or_else(||serde_json::from_str::<AgentResult>("").unwrap_err()))
        .map_err(|_|bad("Agent must return a valid result JSON object on stdout or write it to RELAY_OUTPUT"))?;
    engine::validate_result(&result)?;
    Ok(result)
}
pub fn demo_result(input: &Value) -> AgentResult {
    let stage = input["stage"].as_str().unwrap_or("planning");
    let title = input["task"]["title"].as_str().unwrap_or("Example task");
    AgentResult {
        outcome: "complete".into(),
        summary: format!("Demo {stage} complete — simulated output"),
        send_back_to: None,
        reason: String::new(),
        artifacts: vec![ResultArtifact {
            name: format!("demo-{stage}.md"),
            content: format!(
                "# Demo {stage}\n\nTask: {title}\n\nThis artifact is simulated. No real implementation, review, or test was performed.\n\n## Handoff\n\nThe next stage receives this versioned artifact and the task's acceptance criteria. Configure a CLI agent to perform real work."
            ),
            media_type: "text/markdown".into(),
        }],
        usage: Usage::default(),
        model: Some("deterministic-demo".into()),
        estimate: None,
    }
}
