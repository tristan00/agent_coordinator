mod adapters;
mod agents;
mod analytics;
mod engine;
mod evidence;
mod history;
mod metrics;
mod process;
mod redeploy;
mod telemetry;
#[cfg(test)]
mod test_support;
mod web;
mod workspace_prompts;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition { Ok(()) } else { Err(message.into().into()) }
}
fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}
fn read<T: for<'a> Deserialize<'a>>(file: &Path) -> Result<T> {
    Ok(serde_json::from_slice(&fs::read(file)?)?)
}
fn write<T: Serialize>(file: &Path, value: &T) -> Result<()> {
    fs::create_dir_all(file.parent().unwrap())?;
    let temporary = file.with_extension(format!("{}.tmp", std::process::id()));
    let mut output = File::create(&temporary)?;
    output.write_all(&serde_json::to_vec_pretty(value)?)?;
    output.sync_all()?;
    drop(output);
    fs::rename(temporary, file)?;
    Ok(())
}
fn lock(file: &Path) -> Result<File> {
    fs::create_dir_all(file.parent().unwrap())?;
    let handle = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(file)?;
    handle.try_lock()?;
    Ok(handle)
}
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--identity") => {
            println!("{}", web::deployment());
            Ok(())
        }
        Some("--redeploy") => redeploy::cli(&args[2..]),
        Some("--redeploy-request") => redeploy::request(Path::new(args.get(2).ok_or("Missing request")?)),
        Some("--supervise") => process::supervise(Path::new(args.get(2).ok_or("Missing attempt path")?)),
        Some("--agent") => process::agent(),
        Some("--external") => process::external(),
        Some("--worker") => process::worker(),
        Some("--deploy") => web::deploy(args.get(2).ok_or("Supply a task ID or rollback")?),
        Some("--finish-deploy") => web::finish_deploy(),
        _ => web::serve(),
    }
}
