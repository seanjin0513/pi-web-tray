use std::{
    io::{self, Read},
    process::{Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[cfg(windows)]
use process_wrap::std::{ChildWrapper, CommandWrap};
#[cfg(unix)]
use std::os::unix::process::CommandExt;

pub type Result<T> = std::result::Result<T, String>;
const POLL: Duration = Duration::from_millis(50);
const OUTPUT_LIMIT: usize = 16 * 1024;

#[derive(Default)]
struct Output {
    bytes: Vec<u8>,
    done: bool,
}

fn capture(mut reader: impl Read + Send + 'static) -> Arc<Mutex<Output>> {
    let output = Arc::new(Mutex::new(Output::default()));
    let captured = output.clone();
    thread::spawn(move || {
        let mut chunk = [0; 4096];
        while let Ok(n @ 1..) = reader.read(&mut chunk) {
            let mut output = captured.lock().unwrap();
            let excess = (output.bytes.len() + n).saturating_sub(OUTPUT_LIMIT);
            output.bytes.drain(..excess);
            output.bytes.extend_from_slice(&chunk[..n]);
        }
        captured.lock().unwrap().done = true;
    });
    output
}

pub struct OwnedChild {
    #[cfg(unix)]
    child: std::process::Child,
    #[cfg(windows)]
    child: Box<dyn ChildWrapper>,
    ended: bool,
    stdout: Arc<Mutex<Output>>,
    stderr: Arc<Mutex<Output>>,
}

impl OwnedChild {
    pub fn spawn(mut command: Command) -> io::Result<Self> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        let (child, stdout, stderr) = {
            let mut child = command.process_group(0).spawn()?;
            let stdout = capture(child.stdout.take().expect("piped stdout"));
            let stderr = capture(child.stderr.take().expect("piped stderr"));
            (child, stdout, stderr)
        };
        #[cfg(windows)]
        let (child, stdout, stderr) = {
            use process_wrap::std::{CreationFlags, JobObject, KillOnDrop};
            let mut command = CommandWrap::from(command);
            command.wrap(CreationFlags(0x08000000)); // CREATE_NO_WINDOW
            command.wrap(JobObject);
            command.wrap(KillOnDrop);
            let mut child = command.spawn()?;
            let stdout = capture(child.stdout().take().expect("piped stdout"));
            let stderr = capture(child.stderr().take().expect("piped stderr"));
            (child, stdout, stderr)
        };
        Ok(Self {
            child,
            ended: false,
            stdout,
            stderr,
        })
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        #[cfg(unix)]
        if !self.ended {
            // Keep the exited leader unreaped until after signaling its group:
            // the PID/PGID can't be reused by an unrelated process in that gap.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.child.id() as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    return Ok(None);
                }
                return Err(error);
            }
            if unsafe { info.si_pid() } == 0 {
                return Ok(None);
            }
            if let Err(error) = self.force_kill() {
                // Darwin can return EPERM for a group containing just its zombie leader.
                if !cfg!(target_os = "macos") || error.raw_os_error() != Some(libc::EPERM) {
                    return Err(error);
                }
            }
        }
        let result = self.child.try_wait()?;
        self.ended |= result.is_some();
        Ok(result)
    }

    fn force_kill(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            let result = unsafe { libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL) };
            if result == -1 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
            Ok(())
        }
        #[cfg(windows)]
        self.child.start_kill()
    }

    pub fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.stdout.lock().unwrap().bytes).into_owned()
    }

    pub fn detail(&self) -> String {
        let stderr = self.stderr.lock().unwrap();
        let text = String::from_utf8_lossy(&stderr.bytes);
        let last = text.lines().rev().take(4).collect::<Vec<_>>();
        last.into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
            .chars()
            .take(600)
            .collect()
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> io::Result<Option<ExitStatus>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(Some(status));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            thread::sleep(POLL);
        }
    }

    pub fn stop(&mut self) -> io::Result<()> {
        if self.ended {
            return Ok(());
        }
        #[cfg(unix)]
        {
            // Let the launcher forward TERM to Next.js and clean its run record.
            let result = unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
            if result == -1 && io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                return Err(io::Error::last_os_error());
            }
            if self.wait_for_exit(Duration::from_secs(5))?.is_some() {
                return Ok(());
            }
        }
        self.force_kill()?;
        if self.wait_for_exit(Duration::from_secs(3))?.is_some() {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::TimedOut, "进程未退出"))
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.ended && self.force_kill().is_ok() {
            let _ = self.wait_for_exit(Duration::from_secs(1));
        }
    }
}

pub fn run(command: Command, label: &str, timeout: Duration) -> Result<String> {
    let mut child = OwnedChild::spawn(command).map_err(|error| format!("{label}：{error}"))?;
    let Some(status) = child
        .wait_for_exit(timeout)
        .map_err(|error| format!("{label}：{error}"))?
    else {
        return Err(format!("{label}超时。"));
    };
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        if child.stdout.lock().unwrap().done && child.stderr.lock().unwrap().done {
            break;
        }
        thread::sleep(POLL);
    }
    if !status.success() {
        return Err(format!("{label}失败（{status}）。\n{}", child.detail()));
    }
    Ok(child.stdout())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_a_short_lived_child_after_exit() {
        #[cfg(unix)]
        let command = {
            let mut command = Command::new("/bin/echo");
            command.arg("ok");
            command
        };
        #[cfg(windows)]
        let command = {
            let mut command = Command::new("cmd.exe");
            command.args(["/d", "/c", "echo ok"]);
            command
        };
        assert_eq!(
            run(command, "test", Duration::from_secs(5)).unwrap().trim(),
            "ok"
        );
    }
}
