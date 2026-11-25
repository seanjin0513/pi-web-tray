use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
    process::Command,
    thread,
    time::{Duration, Instant},
};

use crate::process::{OwnedChild, Result};

const ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 30141));
pub const URL: &str = "http://127.0.0.1:30141";

#[derive(Default)]
pub struct Service {
    child: Option<OwnedChild>,
    ready: bool,
}

impl Service {
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn is_started(&self) -> bool {
        self.child.is_some()
    }

    pub fn start(&mut self, command: Command) -> Result<()> {
        self.start_at(command, ADDR, Duration::from_secs(30))
    }

    fn start_at(
        &mut self,
        mut command: Command,
        addr: SocketAddr,
        timeout: Duration,
    ) -> Result<()> {
        if self.is_started() {
            return Err("已有本应用启动的 Pi Web 进程，请先停止。".into());
        }
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return Err(format!("{addr} 已被占用。不会接管或停止其他服务。"));
        }
        command.args([
            "--no-open",
            "--hostname",
            "127.0.0.1",
            "--port",
            &addr.port().to_string(),
        ]);
        self.child = Some(OwnedChild::spawn(command).map_err(|e| format!("无法启动 Pi Web：{e}"))?);
        let deadline = Instant::now() + timeout;
        let error = loop {
            if let Err(error) = self.poll() {
                break error;
            }
            let child = self.child.as_ref().unwrap();
            // A listener on this port alone is not evidence our server started.
            if ready_line(&child.stdout()) && http_ready(addr) {
                self.ready = true;
                return Ok(());
            }
            if Instant::now() >= deadline {
                break format!("Pi Web 启动超时。\n{}", child.detail());
            }
            thread::sleep(Duration::from_millis(150));
        };
        if let Err(stop_error) = self.stop() {
            return Err(format!("{error}\n{stop_error}"));
        }
        Err(error)
    }

    pub fn stop(&mut self) -> Result<()> {
        self.ready = false;
        if let Some(child) = self.child.as_mut() {
            child.stop().map_err(|e| format!("停止 Pi Web 失败：{e}"))?;
        }
        self.child = None;
        Ok(())
    }

    pub fn poll(&mut self) -> Result<()> {
        if let Some(child) = self.child.as_mut()
            && let Some(status) = child
                .try_wait()
                .map_err(|e| format!("检查 Pi Web 进程失败：{e}"))?
        {
            let message = format!("Pi Web 已退出（{status}）。\n{}", child.detail());
            self.child = None;
            self.ready = false;
            return Err(message);
        }
        Ok(())
    }
}

fn ready_line(stdout: &str) -> bool {
    stdout.lines().any(|line| {
        let line = line.trim();
        line.starts_with("✓ Ready in ") || line.starts_with("Ready in ")
    })
}

fn http_ready(addr: SocketAddr) -> bool {
    let timeout = Duration::from_millis(500);
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, timeout) else {
        return false;
    };
    let request = format!("GET / HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    if stream.set_read_timeout(Some(timeout)).is_err()
        || stream.set_write_timeout(Some(timeout)).is_err()
        || stream.write_all(request.as_bytes()).is_err()
    {
        return false;
    }
    let mut first = [0; 12];
    stream.read_exact(&mut first).is_ok()
        && first.starts_with(b"HTTP/1.")
        && matches!(first[9], b'2' | b'3' | b'4')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn never_takes_over_an_occupied_port() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let error = Service::default()
            .start_at(Command::new("should-not-start"), addr, Duration::ZERO)
            .unwrap_err();
        assert!(error.contains("已被占用"));
        assert!(TcpStream::connect(addr).is_ok());
    }

    #[test]
    fn requires_next_ready_line_and_accepts_http_auth() {
        assert!(ready_line("▲ Next.js\n✓ Ready in 187ms"));
        assert!(!ready_line("Not Ready in 187ms"));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let responder = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
        });
        assert!(http_ready(addr));
        responder.join().unwrap();
    }
}
