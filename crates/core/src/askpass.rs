//! The askpass bridge (ADR-003, issue #42): git and ssh ask the user through GitCraft's window.
//!
//! GitCraft sets `GIT_ASKPASS` and `SSH_ASKPASS` (with `SSH_ASKPASS_REQUIRE=force`) to its own
//! executable. Started that way ([`HELPER_TOKEN_ENV`] set, the prompt as the only argument),
//! the executable is a small client ([`client`]): it connects to the window's [`Askpass`]
//! server on the loopback interface, proves itself with the session's random token, sends the
//! prompt and prints the answer for git.
//!
//! - The server listens on `127.0.0.1` only; any local process can connect, so every request
//!   must carry the token (256 random bits, compared in constant time). Without it the
//!   connection is closed and no dialog appears.
//! - Requests are small and bounded (token line, prompt ≤ 16 KiB) and must arrive within a few
//!   seconds; at most a few connections are handled at once.
//! - Answers go to the client only: never logged, stored or kept after they are sent.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::Wake;

/// Set in git's environment: where the window listens.
pub const HELPER_ADDR_ENV: &str = "GITCRAFT_ASKPASS_ADDR";
/// Set in git's environment: the session's token. Its presence starts helper mode.
pub const HELPER_TOKEN_ENV: &str = "GITCRAFT_ASKPASS_TOKEN";

const MAGIC: &str = "GITCRAFT-ASKPASS 1";
const MAX_PROMPT: usize = 16 * 1024;
/// A request must be complete this soon after connecting.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a prompt waits for the user.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Connections handled at the same time (git asks one question at a time).
const MAX_CONNECTIONS: usize = 4;

/// What git or ssh asks for, from the wording of its prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    Username,
    /// A password, token, passphrase or PIN: entered hidden.
    Secret,
    /// ssh does not know the host's key: accept (ssh remembers it) or reject.
    HostKey {
        host: String,
        fingerprint: String,
    },
    /// Another yes/no question.
    Confirm,
}

impl PromptKind {
    pub fn of(prompt: &str) -> PromptKind {
        let lower = prompt.to_lowercase();
        if lower.contains("authenticity of host") && lower.contains("fingerprint") {
            let host = prompt.split('\'').nth(1).unwrap_or_default().to_owned();
            let fingerprint = prompt
                .split_whitespace()
                .find(|w| w.starts_with("SHA256:") || w.starts_with("MD5:"))
                .unwrap_or_default()
                .trim_end_matches('.')
                .to_owned();
            return PromptKind::HostKey { host, fingerprint };
        }
        if lower.contains("username") {
            return PromptKind::Username;
        }
        let secret = ["password", "passphrase", "passcode", "pin", "token", "otp"]
            .iter()
            .any(|w| lower.contains(w));
        if !secret && lower.contains("(yes/no") {
            return PromptKind::Confirm;
        }
        // Unknown wording: hide the input, it may be a secret.
        PromptKind::Secret
    }
}

/// A question waiting for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub id: u64,
    /// The prompt as git or ssh wrote it.
    pub text: String,
    pub kind: PromptKind,
}

/// State shared with the server threads.
struct Shared {
    token: String,
    next: AtomicU64,
    connections: AtomicUsize,
    stop: AtomicBool,
    waiting: Mutex<HashMap<u64, Sender<Option<String>>>>,
    prompts: Mutex<Sender<Prompt>>,
    wake: Wake,
}

/// The window's end of the bridge. Dropping it stops the server.
pub struct Askpass {
    addr: SocketAddr,
    shared: Arc<Shared>,
    prompts: Receiver<Prompt>,
}

impl Askpass {
    /// Listens on a free loopback port with a fresh random token.
    pub fn start(wake: Wake) -> std::io::Result<Askpass> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let (tx, prompts) = channel();
        let shared = Arc::new(Shared {
            token,
            next: AtomicU64::new(0),
            connections: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            waiting: Mutex::new(HashMap::new()),
            prompts: Mutex::new(tx),
            wake,
        });
        let server = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("gitcraft-askpass".into())
            .spawn(move || serve(listener, server))?;
        Ok(Askpass {
            addr,
            shared,
            prompts,
        })
    }

    /// The environment that makes git and ssh ask through `exe` (GitCraft's own executable).
    pub fn env(&self, exe: &Path) -> Vec<(OsString, OsString)> {
        vec![
            ("GIT_ASKPASS".into(), exe.into()),
            ("SSH_ASKPASS".into(), exe.into()),
            ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
            (HELPER_ADDR_ENV.into(), self.addr.to_string().into()),
            (HELPER_TOKEN_ENV.into(), self.shared.token.clone().into()),
        ]
    }

    /// Questions that arrived since the last call.
    pub fn poll(&self) -> Vec<Prompt> {
        self.prompts.try_iter().collect()
    }

    /// Answers a question; `None` refuses it (git then fails with a login error).
    pub fn answer(&self, id: u64, answer: Option<String>) {
        let sender = self
            .shared
            .waiting
            .lock()
            .ok()
            .and_then(|mut w| w.remove(&id));
        if let Some(sender) = sender {
            let _ = sender.send(answer);
        }
    }
}

impl Drop for Askpass {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        // Waiting prompts end refused.
        if let Ok(mut w) = self.shared.waiting.lock() {
            for (_, sender) in w.drain() {
                let _ = sender.send(None);
            }
        }
    }
}

fn serve(listener: TcpListener, shared: Arc<Shared>) {
    while !shared.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                if shared.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    shared.connections.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let s = Arc::clone(&shared);
                let started = std::thread::Builder::new()
                    .name("gitcraft-askpass-request".into())
                    .spawn(move || {
                        let _ = handle(stream, &s);
                        s.connections.fetch_sub(1, Ordering::SeqCst);
                    });
                if started.is_err() {
                    shared.connections.fetch_sub(1, Ordering::SeqCst);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Compares without stopping at the first difference (no timing hint about the token).
fn same(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Reads one line of at most `max` bytes (without the newline).
fn line(reader: &mut impl BufRead, max: usize) -> std::io::Result<String> {
    let mut buf = Vec::new();
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    reader.by_ref().take(limit).read_until(b'\n', &mut buf)?;
    if buf.pop() != Some(b'\n') {
        return Err(std::io::Error::other("request line too long or cut off"));
    }
    String::from_utf8(buf).map_err(|_| std::io::Error::other("request is not UTF-8"))
}

fn handle(stream: TcpStream, shared: &Shared) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    if line(&mut reader, MAGIC.len())? != MAGIC {
        return Ok(());
    }
    let token = line(&mut reader, 128)?;
    if !same(token.as_bytes(), shared.token.as_bytes()) {
        return Ok(());
    }
    let len: usize = line(&mut reader, 8)?
        .parse()
        .map_err(|_| std::io::Error::other("bad length"))?;
    if len > MAX_PROMPT {
        return Ok(());
    }
    let mut text = vec![0u8; len];
    reader.read_exact(&mut text)?;
    let text = String::from_utf8_lossy(&text).into_owned();
    let id = shared.next.fetch_add(1, Ordering::SeqCst) + 1;
    let (tx, rx) = channel();
    if let Ok(mut w) = shared.waiting.lock() {
        w.insert(id, tx);
    }
    let prompt = Prompt {
        id,
        kind: PromptKind::of(&text),
        text,
    };
    if let Ok(p) = shared.prompts.lock() {
        let _ = p.send(prompt);
    }
    (shared.wake)();
    let answer = rx.recv_timeout(ANSWER_TIMEOUT).ok().flatten();
    if let Ok(mut w) = shared.waiting.lock() {
        w.remove(&id);
    }
    let mut out = stream;
    match answer {
        Some(a) if !a.contains(['\n', '\r', '\0']) => {
            out.write_all(b"OK\n")?;
            out.write_all(a.as_bytes())?;
        }
        _ => out.write_all(b"NO\n")?,
    }
    out.flush()
}

/// Sends `prompt` to the window at `addr` with `token`; the answer, or `None` when the user
/// refused.
pub fn request(addr: &str, token: &str, prompt: &str) -> std::io::Result<Option<String>> {
    let addr: SocketAddr = addr
        .parse()
        .map_err(|_| std::io::Error::other("bad address"))?;
    if !addr.ip().is_loopback() {
        return Err(std::io::Error::other("the askpass address is not local"));
    }
    let mut stream = TcpStream::connect_timeout(&addr, REQUEST_TIMEOUT)?;
    let prompt = &prompt.as_bytes()[..prompt.len().min(MAX_PROMPT)];
    let mut req = format!("{MAGIC}\n{token}\n{}\n", prompt.len()).into_bytes();
    req.extend_from_slice(prompt);
    stream.write_all(&req)?;
    stream.flush()?;
    let mut reply = Vec::new();
    stream
        .take(u64::try_from(MAX_PROMPT).unwrap_or(u64::MAX))
        .read_to_end(&mut reply)?;
    let reply = String::from_utf8_lossy(&reply);
    Ok(reply.strip_prefix("OK\n").map(str::to_owned))
}

/// Helper mode: asks the window named in the environment and returns the answer, or `None`
/// when the user refused (or the window is gone).
pub fn client(prompt: &str) -> Option<String> {
    let addr = std::env::var(HELPER_ADDR_ENV).ok()?;
    let token = std::env::var(HELPER_TOKEN_ENV).ok()?;
    request(&addr, &token, prompt).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn server() -> Askpass {
        Askpass::start(Arc::new(|| {})).unwrap()
    }

    fn token_and_addr(a: &Askpass) -> (String, String) {
        let env = a.env(Path::new("gitcraft"));
        let get = |k: &str| {
            env.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.to_string_lossy().into_owned())
                .unwrap()
        };
        (get(HELPER_TOKEN_ENV), get(HELPER_ADDR_ENV))
    }

    fn wait_prompt(a: &Askpass) -> Prompt {
        let start = Instant::now();
        loop {
            if let Some(p) = a.poll().pop() {
                return p;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "no prompt");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_client_with_the_token_gets_the_answer() {
        let a = server();
        let (token, addr) = token_and_addr(&a);
        let asking = std::thread::spawn(move || {
            request(&addr, &token, "Password for 'https://me@example.org': ")
        });
        let p = wait_prompt(&a);
        assert_eq!(p.kind, PromptKind::Secret);
        a.answer(p.id, Some("s3cret".into()));
        assert_eq!(asking.join().unwrap().unwrap(), Some("s3cret".into()));

        let (token, addr) = token_and_addr(&a);
        let asking = std::thread::spawn(move || request(&addr, &token, "Username for 'x': "));
        let p = wait_prompt(&a);
        assert_eq!(p.kind, PromptKind::Username);
        a.answer(p.id, None);
        assert_eq!(asking.join().unwrap().unwrap(), None, "refused");
    }

    #[test]
    fn without_the_token_no_prompt_appears() {
        let a = server();
        let (token, addr) = token_and_addr(&a);
        let mut wrong = token.clone().into_bytes();
        wrong[0] = if wrong[0] == b'a' { b'b' } else { b'a' };
        let wrong = String::from_utf8(wrong).unwrap();
        assert_eq!(request(&addr, &wrong, "Password: ").unwrap(), None);
        assert_eq!(request(&addr, "", "Password: ").unwrap(), None);
        // Garbage instead of a request.
        let mut s = TcpStream::connect(&addr).unwrap();
        s.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
        let mut reply = Vec::new();
        let _ = s.read_to_end(&mut reply);
        assert!(reply.is_empty());
        std::thread::sleep(Duration::from_millis(100));
        assert!(a.poll().is_empty());
        assert!(token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn answers_with_line_breaks_are_refused() {
        let a = server();
        let (token, addr) = token_and_addr(&a);
        let asking = std::thread::spawn(move || request(&addr, &token, "Password: "));
        let p = wait_prompt(&a);
        a.answer(p.id, Some("one\nsecond line".into()));
        assert_eq!(asking.join().unwrap().unwrap(), None);
    }

    #[test]
    fn prompt_kinds() {
        let host = "The authenticity of host 'example.org (192.0.2.1)' can't be established.\nED25519 key fingerprint is SHA256:abcDEF123.\nThis key is not known by any other names.\nAre you sure you want to continue connecting (yes/no/[fingerprint])? ";
        assert_eq!(
            PromptKind::of(host),
            PromptKind::HostKey {
                host: "example.org (192.0.2.1)".into(),
                fingerprint: "SHA256:abcDEF123".into()
            }
        );
        assert_eq!(
            PromptKind::of("Enter passphrase for key '/home/u/.ssh/id_ed25519': "),
            PromptKind::Secret
        );
        assert_eq!(PromptKind::of("Allow? (yes/no) "), PromptKind::Confirm);
        assert_eq!(PromptKind::of("Something new: "), PromptKind::Secret);
    }

    #[test]
    fn only_loopback_addresses_are_used() {
        assert!(request("192.0.2.1:9", "t", "x").is_err());
        assert!(request("nonsense", "t", "x").is_err());
    }
}
