//! Local profile bindings written by the opt-in hcodex .next launchers.
//! The socket directory scopes public pane IDs to a Herdr session. No wire or
//! snapshot changes are needed; public pane IDs already survive restoration.
use std::io;
use std::path::{Path, PathBuf};

#[derive(serde::Deserialize)]
struct Binding {
    version: u32,
    pane_id: String,
    profile: String,
    codex_home: PathBuf,
}

pub(crate) fn valid_profile(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

pub(crate) fn binding_path(socket: &Path, pane_id: &str) -> io::Result<PathBuf> {
    if pane_id.is_empty()
        || pane_id.len() > 128
        || !pane_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b":-_".contains(&c))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid pane ID",
        ));
    }
    let key = pane_id
        .bytes()
        .map(|c| format!("{c:02x}"))
        .collect::<String>();
    let parent = socket
        .parent()
        .ok_or_else(|| io::Error::other("socket has no directory"))?;
    Ok(parent.join("codex-profiles").join(format!("{key}.json")))
}

/// Missing bindings preserve legacy behavior. Broken registered bindings must
/// stop restoration instead of silently falling back to the shared default.
pub(crate) fn apply_binding(
    socket: &Path,
    pane_id: &str,
    argv: &mut Vec<String>,
) -> io::Result<Option<PathBuf>> {
    let path = binding_path(socket, pane_id)?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let binding: Binding = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if binding.version != 1
        || binding.pane_id != pane_id
        || !valid_profile(&binding.profile)
        || !binding.codex_home.is_absolute()
    {
        return Err(io::Error::other("invalid Codex profile binding"));
    }
    let profile_path = binding
        .codex_home
        .join(format!("{}.config.toml", binding.profile));
    if !profile_path.is_file() {
        return Err(io::Error::other(format!(
            "registered Codex profile is missing: {}",
            profile_path.display()
        )));
    }
    let mut selected = None;
    let mut args = argv.iter().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        let value = if arg == "--profile" || arg == "-p" {
            Some(
                args.next()
                    .ok_or_else(|| io::Error::other("missing profile value"))?
                    .as_str(),
            )
        } else {
            arg.strip_prefix("--profile=")
        };
        if let Some(value) = value {
            if selected.replace(value).is_some() || value != binding.profile {
                return Err(io::Error::other(
                    "explicit profile conflicts with pane binding",
                ));
            }
        }
    }
    if selected.is_none() {
        if argv.is_empty() {
            return Err(io::Error::other("empty Codex command"));
        }
        argv.splice(1..1, ["--profile".into(), binding.profile]);
    }
    Ok(Some(binding.codex_home))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_profile_paths_are_session_scoped_and_safe() {
        assert_ne!(
            binding_path(Path::new("/a/herdr.sock"), "w1:p1").unwrap(),
            binding_path(Path::new("/b/herdr.sock"), "w1:p1").unwrap()
        );
        assert!(binding_path(Path::new("/a/herdr.sock"), "../../config").is_err());
        assert!(valid_profile("herdr-c1_2"));
        assert!(!valid_profile("../config"));
        assert!(!valid_profile(""));
    }

    #[test]
    fn codex_profile_restore_uses_binding_and_rejects_missing_profile() {
        let root = std::env::temp_dir().join(format!(
            "herdr-profile-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let socket = root.join("herdr.sock");
        let path = binding_path(&socket, "w1:p1").unwrap();
        let mut argv = vec!["codex".into(), "resume".into(), "saved".into()];
        assert!(apply_binding(&socket, "w1:p1", &mut argv)
            .unwrap()
            .is_none());
        assert_eq!(argv.len(), 3);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec(&serde_json::json!({"version":1,"pane_id":"w1:p1","profile":"slot-one","codex_home":root})).unwrap()).unwrap();
        assert!(apply_binding(&socket, "w1:p1", &mut argv).is_err());
        std::fs::write(root.join("slot-one.config.toml"), "model = 'test'\n").unwrap();
        assert_eq!(
            apply_binding(&socket, "w1:p1", &mut argv).unwrap(),
            Some(root.clone())
        );
        assert_eq!(argv, ["codex", "--profile", "slot-one", "resume", "saved"]);
        let original = argv.clone();
        apply_binding(&socket, "w1:p1", &mut argv).unwrap();
        assert_eq!(argv, original);
        let mut conflict = vec!["codex".into(), "--profile=other".into()];
        assert!(apply_binding(&socket, "w1:p1", &mut conflict).is_err());
        std::fs::write(&path, b"broken json").unwrap();
        assert!(apply_binding(&socket, "w1:p1", &mut Vec::new()).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
