//! snakewood-mcp: an MCP (JSON-RPC over stdio) bridge to the snakewood daemon.
//! Reconnecting thin client — restart it freely without disturbing the world.

use std::io::{BufRead, Write};

use snakewood_daemon::mcp::{dispatch_rpc, JsonRpcRequest, TcpDaemonClient};

fn main() -> std::io::Result<()> {
    let addr = std::env::var("SNAKEWOOD_API_ADDR").unwrap_or_else(|_| "127.0.0.1:4001".to_string());
    let builder =
        std::env::var("SNAKEWOOD_MCP_ACTOR").unwrap_or_else(|_| "player/mcp-builder".to_string());

    let mut client = TcpDaemonClient::connect(&addr, &builder)?;
    eprintln!(
        "snakewood-mcp connected to {addr} as {builder} (session {})",
        client.session
    );

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let req: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("snakewood-mcp: bad JSON-RPC line: {e}");
                continue;
            }
        };
        let session = client.session;
        if let Some(resp) = dispatch_rpc(&req, session, &mut client) {
            let mut out = serde_json::to_string(&resp).unwrap_or_else(|_| {
                "{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32603,\"message\":\"serialize failed\"}}".to_string()
            });
            out.push('\n');
            stdout.write_all(out.as_bytes())?;
            stdout.flush()?;
        }
    }
    Ok(())
}
