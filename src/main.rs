use relay_coordinator::{model, projects, runner};
use std::{
    io::{IsTerminal, Read},
    path::PathBuf,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version") {
        println!("{} {}", env!("CARGO_PKG_VERSION"), env!("RELAY_BUILD_ID"));
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("example-agent") {
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input)?;
        let context: serde_json::Value = serde_json::from_str(&input)?;
        if let Some(index) = args.iter().position(|a| a == "--assert-env-absent") {
            let name = args.get(index + 1).ok_or("Missing environment name")?;
            if std::env::var_os(name).is_some() {
                return Err(format!("Environment variable {name} was not removed").into());
            }
        }
        if let Some(index) = args.iter().position(|a| a == "--delay-ms") {
            let delay: u64 = args.get(index + 1).ok_or("Missing delay")?.parse()?;
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
        if args.iter().any(|a| a == "--fail") {
            return Err("Example agent failure requested".into());
        }
        println!("{}", serde_json::to_string(&runner::demo_result(&context))?);
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Relay — agent coordinator\n\n  relay-coordinator [--port 4242] [--data-dir ./data] [--approval-code-stdin]\n\nThe server binds to 127.0.0.1. Supports local and cloud-backed agents."
        );
        return Ok(());
    }
    let mut port = 4242u16;
    let mut data_dir = PathBuf::from("data");
    let mut approval_code_stdin = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                i += 1;
                port = args.get(i).ok_or("--port needs a value")?.parse()?;
            }
            "--data-dir" => {
                i += 1;
                data_dir = PathBuf::from(args.get(i).ok_or("--data-dir needs a value")?);
            }
            "--approval-code-stdin" => approval_code_stdin = true,
            unknown => return Err(format!("Unknown argument: {unknown}").into()),
        }
        i += 1;
    }
    if port == 0 {
        return Err("Choose a port from 1–65535".into());
    }
    let interactive_approval = !approval_code_stdin && std::io::stdout().is_terminal();
    let approval_code = if approval_code_stdin {
        let mut code = String::new();
        std::io::stdin().read_line(&mut code)?;
        let code = code.trim();
        if code.len() < 24 || !code.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(
                "Supply at least 24 printable non-space ASCII characters as the approval code"
                    .into(),
            );
        }
        code.to_string()
    } else if interactive_approval {
        model::id()
    } else {
        String::new()
    };
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let host = projects::Host::open(&data_dir, port, approval_code.clone())?;
    println!(
        "Relay is ready at http://127.0.0.1:{port}\nData: {}",
        data_dir.display()
    );
    if interactive_approval {
        println!(
            "Operator approval code (this process only): {}",
            approval_code
        );
    }
    axum::serve(listener, projects::router(host.clone()))
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            host.stop();
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        })
        .await?;
    Ok(())
}
