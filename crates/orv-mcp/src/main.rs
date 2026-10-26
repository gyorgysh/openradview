// SPDX-License-Identifier: GPL-3.0-or-later
//! Companion process: stdout is exclusively MCP protocol output.
use zeroize::Zeroizing;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|a| a == "--update-health-check") {
        return if args.len() == 2 && args[1] == env!("CARGO_PKG_VERSION") {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::FAILURE
        };
    }

    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            // Static, sanitized errors only: never print arguments, URLs or tokens.
            eprintln!("OpenRadView MCP: {message}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), &'static str> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["instances", "--json"] || args == ["instances"] {
        let instances = orv_mcp::discovery::instances()?;
        let text = serde_json::to_string_pretty(&serde_json::json!({"instances":instances}))
            .map_err(|_| "Cannot encode local instances")?;
        println!("{text}");
        return Ok(());
    }
    if args == ["connect"] {
        return orv_mcp::connector::serve(None).await;
    }
    if args.len() == 3 && args[0] == "connect" && args[1] == "--broker-url" {
        return orv_mcp::connector::serve(Some(args[2].clone())).await;
    }
    if args.len() != 3 || args[0] != "stdio" || args[1] != "--url" {
        return Err(
            "Usage: openradview-mcp connect | instances --json | stdio --url http://127.0.0.1:PORT/mcp; set OPENRADVIEW_TOKEN from an explicitly consented desktop session",
        );
    }
    orv_mcp::stdio::validate_endpoint(&args[2])?;
    let token = Zeroizing::new(
        std::env::var("OPENRADVIEW_TOKEN").map_err(|_| "OPENRADVIEW_TOKEN is required")?,
    );
    orv_mcp::stdio::serve(args[2].clone(), token).await
}
