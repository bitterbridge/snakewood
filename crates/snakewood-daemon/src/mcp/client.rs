use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

use crate::api::{ApiRequest, ApiResponse};
use crate::mcp::DaemonClient;

/// A synchronous line-delimited-JSON client to the daemon command API, holding a
/// persistent connection bound to a named builder actor, reconnecting on failure.
pub struct TcpDaemonClient {
    addr: String,
    builder: String,
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    pub session: u64,
}

fn send_line(writer: &mut TcpStream, req: &ApiRequest) -> std::io::Result<()> {
    let mut line = serde_json::to_string(req).map_err(to_io)?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

fn read_response(reader: &mut BufReader<TcpStream>) -> std::io::Result<ApiResponse> {
    let mut line = String::new();
    let n = reader.read_line(&mut line)?;
    if n == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "daemon closed",
        ));
    }
    serde_json::from_str(line.trim_end()).map_err(to_io)
}

fn to_io<E: std::fmt::Display>(e: E) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
}

impl TcpDaemonClient {
    /// Connect and attach to the named builder; returns the client with its session.
    pub fn connect(addr: &str, builder: &str) -> std::io::Result<TcpDaemonClient> {
        let stream = TcpStream::connect(addr)?;
        let writer = stream.try_clone()?;
        let mut reader = BufReader::new(stream);
        let mut writer2 = writer;
        send_line(
            &mut writer2,
            &ApiRequest::ConnectAs {
                actor: builder.to_string(),
            },
        )?;
        let session = match read_response(&mut reader)? {
            ApiResponse::Connected { session, .. } => session,
            other => return Err(to_io(format!("expected Connected, got {other:?}"))),
        };
        Ok(TcpDaemonClient {
            addr: addr.to_string(),
            builder: builder.to_string(),
            reader,
            writer: writer2,
            session,
        })
    }

    fn reconnect(&mut self) -> std::io::Result<()> {
        let fresh = TcpDaemonClient::connect(&self.addr, &self.builder)?;
        self.reader = fresh.reader;
        self.writer = fresh.writer;
        self.session = fresh.session;
        Ok(())
    }
}

impl DaemonClient for TcpDaemonClient {
    fn request(&mut self, req: ApiRequest) -> std::io::Result<ApiResponse> {
        // Try once; on I/O failure, reconnect (re-ConnectAs) and retry once.
        match send_line(&mut self.writer, &req).and_then(|()| read_response(&mut self.reader)) {
            Ok(resp) => Ok(resp),
            Err(_) => {
                self.reconnect()?;
                // Re-issue with the (possibly new) session id patched in for
                // session-scoped requests.
                let req = with_session(req, self.session);
                send_line(&mut self.writer, &req)?;
                read_response(&mut self.reader)
            }
        }
    }
}

/// Replace the session id in a session-scoped request (used after reconnect).
fn with_session(req: ApiRequest, session: u64) -> ApiRequest {
    match req {
        ApiRequest::Look { .. } => ApiRequest::Look { session },
        ApiRequest::Move { direction, .. } => ApiRequest::Move { session, direction },
        ApiRequest::Dig {
            direction,
            id,
            name,
            description,
            ..
        } => ApiRequest::Dig {
            session,
            direction,
            id,
            name,
            description,
        },
        ApiRequest::Disconnect { .. } => ApiRequest::Disconnect { session },
        other => other, // Connect / ConnectAs carry no session
    }
}
