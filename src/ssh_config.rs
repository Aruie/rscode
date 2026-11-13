//! Minimal ~/.ssh/config reader: Host blocks, first-match-wins options, Include.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostEntry {
    pub alias: String,
    pub hostname: String,
    pub user: Option<String>,
    pub port: u16,
    pub identity_files: Vec<PathBuf>,
    pub proxy_jump: Option<String>,
}

impl HostEntry {
    pub fn detail(&self) -> String {
        let user = self.user.as_deref().map(|u| format!("{u}@")).unwrap_or_default();
        let port = if self.port != 22 { format!(":{}", self.port) } else { String::new() };
        format!("{user}{}{port}", self.hostname)
    }

    /// Parse an ad-hoc "user@host:port" typed into the host picker.
    pub fn ad_hoc(spec: &str) -> Option<HostEntry> {
        let spec = spec.trim().trim_start_matches("ssh ");
        if spec.is_empty() || spec.contains(char::is_whitespace) {
            return None;
        }
        let (user, rest) = match spec.split_once('@') {
            Some((u, r)) => (Some(u.to_string()), r),
            None => (None, spec),
        };
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) if p.parse::<u16>().is_ok() => (h, p.parse().unwrap()),
            _ => (rest, 22),
        };
        if host.is_empty() {
            return None;
        }
        Some(HostEntry {
            alias: spec.to_string(),
            hostname: host.to_string(),
            user,
            port,
            identity_files: Vec::new(),
            proxy_jump: None,
        })
    }
}

struct Block {
    patterns: Vec<String>,
    opts: Vec<(String, String)>,
}

pub fn ssh_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".ssh"))
}

pub fn config_path() -> Option<PathBuf> {
    ssh_dir().map(|d| d.join("config"))
}

fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        if let Some(h) = dirs::home_dir() {
            return h.join(rest);
        }
    }
    PathBuf::from(s)
}

fn parse_file(path: &Path, blocks: &mut Vec<Block>, depth: usize) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    parse_text(&text, blocks, depth);
}

fn split_kv(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let idx = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let key = line[..idx].to_ascii_lowercase();
    let val = line[idx..].trim_start_matches(|c: char| c.is_whitespace() || c == '=').trim();
    let val = val.trim_matches('"').to_string();
    Some((key, val))
}

fn parse_text(text: &str, blocks: &mut Vec<Block>, depth: usize) {
    for line in text.lines() {
        let Some((key, val)) = split_kv(line) else { continue };
        match key.as_str() {
            "host" => blocks.push(Block { patterns: val.split_whitespace().map(String::from).collect(), opts: Vec::new() }),
            "match" => blocks.push(Block { patterns: vec!["!*".into()], opts: Vec::new() }), // unsupported: never matches
            "include" if depth < 5 => {
                for part in val.split_whitespace() {
                    let mut p = expand_tilde(part);
                    if p.is_relative() {
                        if let Some(d) = ssh_dir() {
                            p = d.join(p);
                        }
                    }
                    // simple "*" globs in the file name part
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    if name.contains('*') {
                        let Some(Ok(rd)) = p.parent().map(std::fs::read_dir) else { continue };
                        let mut files: Vec<PathBuf> = rd
                            .flatten()
                            .map(|e| e.path())
                            .filter(|f| f.file_name().is_some_and(|n| glob_match(&name, &n.to_string_lossy())))
                            .collect();
                        files.sort();
                        for f in files {
                            parse_file(&f, blocks, depth + 1);
                        }
                    } else {
                        parse_file(&p, blocks, depth + 1);
                    }
                }
            }
            _ => {
                if blocks.is_empty() {
                    blocks.push(Block { patterns: vec!["*".into()], opts: Vec::new() });
                }
                blocks.last_mut().unwrap().opts.push((key, val));
            }
        }
    }
}

pub fn glob_match(pat: &str, s: &str) -> bool {
    fn rec(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(b'*'), _) => rec(&p[1..], s) || (!s.is_empty() && rec(p, &s[1..])),
            (Some(b'?'), Some(_)) => rec(&p[1..], &s[1..]),
            (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => rec(&p[1..], &s[1..]),
            _ => false,
        }
    }
    rec(pat.as_bytes(), s.as_bytes())
}

fn block_matches(b: &Block, alias: &str) -> bool {
    let mut matched = false;
    for p in &b.patterns {
        if let Some(neg) = p.strip_prefix('!') {
            if glob_match(neg, alias) {
                return false;
            }
        } else if glob_match(p, alias) {
            matched = true;
        }
    }
    matched
}

fn resolve(blocks: &[Block], alias: &str) -> HostEntry {
    let mut e = HostEntry {
        alias: alias.to_string(),
        hostname: alias.to_string(),
        user: None,
        port: 22,
        identity_files: Vec::new(),
        proxy_jump: None,
    };
    let (mut host_set, mut user_set, mut port_set) = (false, false, false);
    for b in blocks.iter().filter(|b| block_matches(b, alias)) {
        for (k, v) in &b.opts {
            match k.as_str() {
                "hostname" if !host_set => {
                    e.hostname = v.replace("%h", alias);
                    host_set = true;
                }
                "user" if !user_set => {
                    e.user = Some(v.clone());
                    user_set = true;
                }
                "port" if !port_set => {
                    if let Ok(p) = v.parse() {
                        e.port = p;
                        port_set = true;
                    }
                }
                // IdentityFile accumulates, like OpenSSH
                "identityfile" => {
                    let p = expand_tilde(&v.replace("%d", &dirs::home_dir().unwrap_or_default().to_string_lossy()));
                    if !e.identity_files.contains(&p) {
                        e.identity_files.push(p);
                    }
                }
                "proxyjump" if e.proxy_jump.is_none() && v != "none" => e.proxy_jump = Some(v.clone()),
                _ => {}
            }
        }
    }
    e
}

/// Hosts listed in ~/.ssh/config (aliases without wildcards), in file order.
pub fn load_hosts() -> Vec<HostEntry> {
    let Some(path) = config_path() else { return Vec::new() };
    let mut blocks = Vec::new();
    parse_file(&path, &mut blocks, 0);
    hosts_from_blocks(&blocks)
}

fn hosts_from_blocks(blocks: &[Block]) -> Vec<HostEntry> {
    let mut seen = Vec::new();
    for b in blocks {
        for p in &b.patterns {
            if !p.contains(['*', '?', '!']) && !seen.contains(p) {
                seen.push(p.clone());
            }
        }
    }
    seen.iter().map(|a| resolve(blocks, a)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks_first_match_wins() {
        let cfg = "\
# comment
Host dev box
    HostName 10.0.0.5
    User ec2-user
    Port 2222
    IdentityFile ~/.ssh/dev.pem

Host *
    User root
    IdentityFile ~/.ssh/id_ed25519
";
        let mut blocks = Vec::new();
        parse_text(cfg, &mut blocks, 0);
        let hosts = hosts_from_blocks(&blocks);
        assert_eq!(hosts.len(), 2);
        let dev = &hosts[0];
        assert_eq!(dev.alias, "dev");
        assert_eq!(dev.hostname, "10.0.0.5");
        assert_eq!(dev.user.as_deref(), Some("ec2-user"));
        assert_eq!(dev.port, 2222);
        assert_eq!(dev.identity_files.len(), 2);
    }

    #[test]
    fn ad_hoc_spec() {
        let h = HostEntry::ad_hoc("me@example.com:2200").unwrap();
        assert_eq!(h.user.as_deref(), Some("me"));
        assert_eq!(h.hostname, "example.com");
        assert_eq!(h.port, 2200);
        assert!(HostEntry::ad_hoc("").is_none());
    }
}
