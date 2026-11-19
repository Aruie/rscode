//! Extension host: finds `extensions/<id>/extension.txt`, loads the library of an extension only
//! while it is enabled, and passes JSON requests through its C entry points.

use rcode_ext::{Contributes, Request, Response, API_VERSION};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub struct Ext {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub publisher: String,
    dir: PathBuf,
    library: String,
    lib: Option<Loaded>,
    pub contributes: Contributes,
    pub error: Option<String>,
}

struct Loaded {
    call: unsafe extern "C" fn(*const u8, usize, *mut usize) -> *mut u8,
    free: unsafe extern "C" fn(*mut u8, usize),
}

pub fn extensions_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("RCODE_EXTENSIONS") {
        return Some(PathBuf::from(p));
    }
    Some(std::env::current_exe().ok()?.parent()?.join("extensions"))
}

fn lib_file(dir: &Path, name: &str) -> PathBuf {
    if cfg!(windows) {
        dir.join(format!("{name}.dll"))
    } else if cfg!(target_os = "macos") {
        dir.join(format!("lib{name}.dylib"))
    } else {
        dir.join(format!("lib{name}.so"))
    }
}

pub fn discover() -> Vec<Ext> {
    let Some(root) = extensions_dir() else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(&root) else { return Vec::new() };
    let mut v: Vec<Ext> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let dir = e.path();
            let text = std::fs::read_to_string(dir.join("extension.txt")).ok()?;
            let id = dir.file_name()?.to_string_lossy().into_owned();
            let mut x = Ext {
                name: id.clone(),
                library: id.replace('-', "_"),
                id,
                description: String::new(),
                version: String::new(),
                publisher: String::new(),
                dir,
                lib: None,
                contributes: Contributes::default(),
                error: None,
            };
            for line in text.lines() {
                let Some((k, val)) = line.split_once('=') else { continue };
                let val = val.trim().to_string();
                match k.trim() {
                    "name" => x.name = val,
                    "description" => x.description = val,
                    "version" => x.version = val,
                    "publisher" => x.publisher = val,
                    "library" => x.library = val,
                    _ => {}
                }
            }
            Some(x)
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

impl Ext {
    pub fn is_loaded(&self) -> bool {
        self.lib.is_some()
    }

    /// Load the library and activate it. The library stays mapped until the editor exits
    /// (unloading code that may still be referenced is not worth the risk).
    pub fn activate(&mut self) -> Result<(), String> {
        if self.lib.is_none() {
            let path = lib_file(&self.dir, &self.library);
            // SAFETY: the extension library is trusted code the user put in the extensions folder.
            let loaded = unsafe {
                let lib = libloading::Library::new(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                let lib: &'static libloading::Library = Box::leak(Box::new(lib));
                let ver: libloading::Symbol<unsafe extern "C" fn() -> u32> =
                    lib.get(b"rcode_ext_api_version").map_err(|e| format!("not an rcode extension: {e}"))?;
                let v = ver();
                if v != API_VERSION {
                    return Err(format!("built for extension API {v}, this rcode supports {API_VERSION}"));
                }
                let call = *lib.get::<unsafe extern "C" fn(*const u8, usize, *mut usize) -> *mut u8>(b"rcode_ext_call").map_err(|e| e.to_string())?;
                let free = *lib.get::<unsafe extern "C" fn(*mut u8, usize)>(b"rcode_ext_free").map_err(|e| e.to_string())?;
                Loaded { call, free }
            };
            self.lib = Some(loaded);
        }
        let r = self.call(&Request::Activate { api: API_VERSION })?;
        self.contributes = r.contributes.unwrap_or_default();
        Ok(())
    }

    pub fn deactivate(&mut self) {
        self.contributes = Contributes::default();
    }

    pub fn call(&self, req: &Request) -> Result<Response, String> {
        let lib = self.lib.as_ref().ok_or("extension is not loaded")?;
        let input = serde_json::to_vec(req).map_err(|e| e.to_string())?;
        let mut len = 0usize;
        // SAFETY: entry points follow the rcode_ext ABI; the returned buffer is freed by its owner.
        let bytes = unsafe {
            let p = (lib.call)(input.as_ptr(), input.len(), &mut len);
            if p.is_null() {
                return Err("extension returned nothing".into());
            }
            let v = std::slice::from_raw_parts(p, len).to_vec();
            (lib.free)(p, len);
            v
        };
        let r: Response = serde_json::from_slice(&bytes).map_err(|e| format!("bad response: {e}"))?;
        match r.error {
            Some(e) => Err(e),
            None => Ok(r),
        }
    }
}

// ---------------------------------------------------------------- settings

fn settings_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("rcode").join("settings.txt"))
}

#[derive(Default)]
pub struct Settings {
    pub enabled_extensions: BTreeSet<String>,
    pub disabled_grammars: BTreeSet<String>,
}

impl Settings {
    pub fn load() -> Self {
        let mut s = Settings::default();
        let Some(text) = settings_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return s };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let set: BTreeSet<String> = v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
            match k.trim() {
                "extensions.enabled" => s.enabled_extensions = set,
                "grammars.disabled" => s.disabled_grammars = set,
                _ => {}
            }
        }
        s
    }

    pub fn save(&self) -> Result<(), String> {
        let p = settings_path().ok_or("no config directory")?;
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let join = |s: &BTreeSet<String>| s.iter().cloned().collect::<Vec<_>>().join(", ");
        let text = format!(
            "# rcode settings\nextensions.enabled = {}\ngrammars.disabled = {}\n",
            join(&self.enabled_extensions),
            join(&self.disabled_grammars)
        );
        std::fs::write(&p, text).map_err(|e| e.to_string())
    }
}

/// Match a VS Code style key spec part like "Ctrl+Shift+V" against a pressed key.
pub fn key_part_matches(part: &str, key: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
    let mut want = (false, false, false);
    let mut k = "";
    for p in part.split('+') {
        match p.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "cmd" => want.0 = true,
            "shift" => want.1 = true,
            "alt" => want.2 = true,
            _ => k = p.trim(),
        }
    }
    want == (ctrl, shift, alt) && !k.is_empty() && k.eq_ignore_ascii_case(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        assert!(key_part_matches("Ctrl+Shift+V", "v", true, true, false));
        assert!(!key_part_matches("Ctrl+Shift+V", "v", true, false, false));
        assert!(key_part_matches("V", "v", false, false, false));
    }
}
