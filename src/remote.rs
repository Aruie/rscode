//! SSH + SFTP connection running on its own thread with a small tokio runtime.
//! The UI sends `Req`s; results come back as `Event`s through the `emit` callback.

use crate::ssh_config::HostEntry;
use russh::client;
use russh::keys::{self, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

pub enum Req {
    ReadDir { path: String },
    ReadFile { path: String },
    WriteFile { path: String, data: Vec<u8>, hash: u64 },
    CreateFile { path: String },
    CreateDir { path: String },
    Remove { path: String, is_dir: bool },
    Rename { from: String, to: String },
    IsDir { path: String },
    Walk { root: String },
    Disconnect,
}

pub enum Event {
    Status(String),
    HostKeyUnknown { host: String, fingerprint: String, reply: oneshot::Sender<bool> },
    Prompt { title: String, password: bool, reply: oneshot::Sender<Option<String>> },
    Connected { home: String },
    Failed(String),
    Dir { path: String, result: Result<Vec<(String, bool)>, String> },
    File { path: String, result: Result<Vec<u8>, String> },
    Written { path: String, hash: u64, result: Result<(), String> },
    Done { op: &'static str, path: String, result: Result<(), String> },
    IsDir { path: String, result: Result<bool, String> },
    Walked { root: String, files: Vec<String>, truncated: bool },
    Closed(String),
}

pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

const MAX_FILE: u64 = 20 * 1024 * 1024;
const WALK_LIMIT: usize = 20_000;
pub const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", ".venv", "venv", "__pycache__", ".cache", ".idea", ".vs"];

pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") }
}

pub fn parent(path: &str) -> String {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) => "/".into(),
        Some((p, _)) => p.into(),
        None => path.into(),
    }
}

struct Handler {
    host: String,
    port: u16,
    emit: Emit,
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            (self.emit)(Event::Status("Host certificates are not supported.".into()));
            return Ok(false);
        };
        match keys::check_known_hosts(&self.host, self.port, key) {
            Ok(true) => Ok(true),
            Ok(false) => {
                let (tx, rx) = oneshot::channel();
                let fingerprint = key.fingerprint(keys::HashAlg::Sha256).to_string();
                (self.emit)(Event::HostKeyUnknown { host: self.host.clone(), fingerprint, reply: tx });
                if rx.await.unwrap_or(false) {
                    if let Err(e) = keys::known_hosts::learn_known_hosts(&self.host, self.port, key) {
                        (self.emit)(Event::Status(format!("Could not update known_hosts: {e}")));
                    }
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Err(keys::Error::KeyChanged { line }) => {
                (self.emit)(Event::Failed(format!(
                    "WARNING: the host key for '{}' has changed (known_hosts line {line}). Connection refused. Remove the old entry if this change is expected.",
                    self.host
                )));
                Ok(false)
            }
            Err(e) => {
                // unreadable known_hosts: treat as unknown host
                let (tx, rx) = oneshot::channel();
                let fingerprint = key.fingerprint(keys::HashAlg::Sha256).to_string();
                (self.emit)(Event::Status(format!("known_hosts: {e}")));
                (self.emit)(Event::HostKeyUnknown { host: self.host.clone(), fingerprint, reply: tx });
                Ok(rx.await.unwrap_or(false))
            }
        }
    }
}

async fn prompt(emit: &Emit, title: String, password: bool) -> Option<String> {
    let (tx, rx) = oneshot::channel();
    emit(Event::Prompt { title, password, reply: tx });
    rx.await.ok().flatten()
}

fn default_user() -> String {
    std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "root".into())
}

fn candidate_keys(host: &HostEntry) -> Vec<std::path::PathBuf> {
    let mut v = host.identity_files.clone();
    if let Some(dir) = crate::ssh_config::ssh_dir() {
        for n in ["id_ed25519", "id_ecdsa", "id_rsa"] {
            let p = dir.join(n);
            if !v.contains(&p) {
                v.push(p);
            }
        }
    }
    v.into_iter().filter(|p| p.is_file()).collect()
}

async fn authenticate(handle: &mut client::Handle<Handler>, host: &HostEntry, user: &str, emit: &Emit) -> Result<bool, String> {
    for path in candidate_keys(host) {
        let Ok(pem) = std::fs::read_to_string(&path) else { continue };
        let key = match keys::decode_secret_key(&pem, None) {
            Ok(k) => k,
            Err(keys::Error::KeyIsEncrypted) => {
                let title = format!("Enter passphrase for key '{}'", path.display());
                let Some(pass) = prompt(emit, title, true).await else { continue };
                match keys::decode_secret_key(&pem, Some(&pass)) {
                    Ok(k) => k,
                    Err(e) => {
                        emit(Event::Status(format!("Could not decrypt {}: {e}", path.display())));
                        continue;
                    }
                }
            }
            Err(_) => continue,
        };
        let hash = handle.best_supported_rsa_hash().await.map_err(|e| e.to_string())?.flatten();
        let res = handle
            .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
            .await
            .map_err(|e| e.to_string())?;
        if res.success() {
            return Ok(true);
        }
    }
    for _ in 0..3 {
        let title = format!("Enter password for {user}@{}", host.hostname);
        let Some(pass) = prompt(emit, title, true).await else { return Ok(false) };
        let res = handle.authenticate_password(user, pass).await.map_err(|e| e.to_string())?;
        if res.success() {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn connect(host: HostEntry, emit: Emit) -> mpsc::UnboundedSender<Req> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("ssh".into())
        .stack_size(512 * 1024)
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    emit(Event::Failed(e.to_string()));
                    return;
                }
            };
            rt.block_on(async move {
                match session(host, rx, emit.clone()).await {
                    Ok(reason) => emit(Event::Closed(reason)),
                    Err(e) => emit(Event::Failed(e)),
                }
            });
        })
        .expect("spawn ssh thread");
    tx
}

async fn session(host: HostEntry, mut rx: mpsc::UnboundedReceiver<Req>, emit: Emit) -> Result<String, String> {
    if host.proxy_jump.is_some() {
        emit(Event::Status("ProxyJump is not supported yet; connecting directly.".into()));
    }
    let config = Arc::new(client::Config {
        inactivity_timeout: None,
        keepalive_interval: Some(std::time::Duration::from_secs(30)),
        keepalive_max: 4,
        ..Default::default()
    });
    let handler = Handler { host: host.hostname.clone(), port: host.port, emit: emit.clone() };
    emit(Event::Status(format!("Connecting to {}...", host.detail())));
    let addr = (host.hostname.as_str(), host.port);
    let mut handle = tokio::time::timeout(std::time::Duration::from_secs(20), client::connect(config, addr, handler))
        .await
        .map_err(|_| format!("Connection to {} timed out.", host.hostname))?
        .map_err(|e| format!("Could not establish connection to \"{}\": {e}", host.alias))?;

    let user = host.user.clone().unwrap_or_else(default_user);
    if !authenticate(&mut handle, &host, &user, &emit).await? {
        return Err(format!("Permission denied for {user}@{}.", host.hostname));
    }
    emit(Event::Status("Starting SFTP session...".into()));
    let channel = handle.channel_open_session().await.map_err(|e| e.to_string())?;
    channel.request_subsystem(true, "sftp").await.map_err(|e| e.to_string())?;
    let sftp = Arc::new(SftpSession::new(channel.into_stream()).await.map_err(|e| format!("SFTP: {e}"))?);
    let home = sftp.canonicalize(".").await.unwrap_or_else(|_| "/".into());
    emit(Event::Connected { home });

    while let Some(req) = rx.recv().await {
        if handle.is_closed() {
            return Ok("Connection lost.".into());
        }
        if let Req::Disconnect = req {
            let _ = handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
            return Ok("Disconnected.".into());
        }
        // Each request runs as its own task so a long walk doesn't block file reads.
        let (sftp, emit) = (sftp.clone(), emit.clone());
        tokio::spawn(async move { serve(req, &sftp, &emit).await });
    }
    Ok("Disconnected.".into())
}

fn es<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

async fn serve(req: Req, sftp: &SftpSession, emit: &Emit) {
    match req {
        Req::ReadDir { path } => {
            let result = read_dir(sftp, &path).await;
            emit(Event::Dir { path, result });
        }
        Req::ReadFile { path } => {
            let result = async {
                let meta = sftp.metadata(path.clone()).await.map_err(es)?;
                if meta.size.unwrap_or(0) > MAX_FILE {
                    return Err("The file is too large to open (over 20 MB).".to_string());
                }
                sftp.read(path.clone()).await.map_err(es)
            }
            .await;
            emit(Event::File { path, result });
        }
        Req::WriteFile { path, data, hash } => {
            let result = async {
                // SftpSession::write does not truncate, so open with TRUNCATE ourselves.
                let mut f = sftp
                    .open_with_flags(path.clone(), OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE)
                    .await
                    .map_err(es)?;
                f.write_all(&data).await.map_err(es)?;
                f.shutdown().await.map_err(es)?;
                Ok(())
            }
            .await;
            emit(Event::Written { path, hash, result });
        }
        Req::CreateFile { path } => {
            let result = async {
                if sftp.try_exists(path.clone()).await.map_err(es)? {
                    return Err(format!("A file or folder {path} already exists."));
                }
                let f = sftp.create(path.clone()).await.map_err(es)?;
                drop(f);
                Ok(())
            }
            .await;
            emit(Event::Done { op: "create-file", path, result });
        }
        Req::CreateDir { path } => {
            let result = sftp.create_dir(path.clone()).await.map_err(es);
            emit(Event::Done { op: "create-dir", path, result });
        }
        Req::Remove { path, is_dir } => {
            let result = if is_dir { remove_tree(sftp, &path).await } else { sftp.remove_file(path.clone()).await.map_err(es) };
            emit(Event::Done { op: "remove", path, result });
        }
        Req::Rename { from, to } => {
            let result = sftp.rename(from.clone(), to.clone()).await.map_err(es);
            emit(Event::Done { op: "rename", path: format!("{from}\n{to}"), result });
        }
        Req::IsDir { path } => {
            let result = sftp.metadata(path.clone()).await.map(|m| m.is_dir()).map_err(es);
            emit(Event::IsDir { path, result });
        }
        Req::Walk { root } => {
            let (files, truncated) = walk(sftp, &root).await;
            emit(Event::Walked { root, files, truncated });
        }
        Req::Disconnect => {}
    }
}

async fn read_dir(sftp: &SftpSession, path: &str) -> Result<Vec<(String, bool)>, String> {
    let mut out = Vec::new();
    for e in sftp.read_dir(path.to_string()).await.map_err(es)? {
        let name = e.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let ft = e.file_type();
        let is_dir = if ft.is_symlink() {
            sftp.metadata(join(path, &name)).await.map(|m| m.is_dir()).unwrap_or(false)
        } else {
            ft.is_dir()
        };
        out.push((name, is_dir));
    }
    Ok(out)
}

async fn remove_tree(sftp: &SftpSession, path: &str) -> Result<(), String> {
    // iterative post-order delete
    let mut stack = vec![(path.to_string(), false)];
    while let Some((p, visited)) = stack.pop() {
        if visited {
            sftp.remove_dir(p).await.map_err(es)?;
            continue;
        }
        stack.push((p.clone(), true));
        for e in sftp.read_dir(p.clone()).await.map_err(es)? {
            let name = e.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let child = join(&p, &name);
            if e.file_type().is_dir() {
                stack.push((child, false));
            } else {
                sftp.remove_file(child).await.map_err(es)?;
            }
        }
    }
    Ok(())
}

async fn walk(sftp: &SftpSession, root: &str) -> (Vec<String>, bool) {
    let mut files = Vec::new();
    let mut queue = VecDeque::from([String::new()]);
    while let Some(rel) = queue.pop_front() {
        let dir = if rel.is_empty() { root.to_string() } else { join(root, &rel) };
        let Ok(rd) = sftp.read_dir(dir).await else { continue };
        for e in rd {
            let name = e.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            if e.file_type().is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) {
                    queue.push_back(r);
                }
            } else {
                files.push(r);
                if files.len() >= WALK_LIMIT {
                    return (files, true);
                }
            }
        }
    }
    (files, false)
}
