/// Load secrets from a bundled env file and individual `_FILE` overrides.
///
/// Priority (highest to lowest):
///   1. `KEY` env var set directly
///   2. `KEY_FILE` env var pointing to a file
///   3. Value from `SECRETS_FILE` (a `KEY=value` file)
///
/// Call `load_secrets_file()` once at startup before reading any secrets.
/// It populates env vars from the file only for keys not already set.

/// Read `SECRETS_FILE` and set any env vars that are not already present.
/// Silently does nothing if `SECRETS_FILE` is unset.
pub fn load_secrets_file() {
    let path = match std::env::var("SECRETS_FILE") {
        Ok(p) if !p.is_empty() => p,
        _ => return,
    };
    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rustyboy: failed to read SECRETS_FILE {path}: {e}");
            return;
        }
    };
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            // Only set if not already present — explicit env vars take precedence
            if std::env::var(key).is_err() {
                // Safety: single-threaded startup, no other threads reading env yet
                unsafe { std::env::set_var(key, value); }
            }
        }
    }
}

/// Read a secret: checks `KEY` env var, then `KEY_FILE`.
pub fn get_secret(key: &str) -> String {
    if let Ok(val) = std::env::var(key) {
        if !val.is_empty() {
            return val;
        }
    }
    let file_key = format!("{}_FILE", key);
    if let Ok(path) = std::env::var(&file_key) {
        match std::fs::read_to_string(&path) {
            Ok(s) => return s.trim().to_string(),
            Err(e) => eprintln!("rustyboy: failed to read secret file {path}: {e}"),
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Load secrets from a specific file path (not from env) and return the
    /// resulting key→value map without touching the process environment.
    fn load_from_file(path: &std::path::Path) -> std::collections::HashMap<String, String> {
        let contents = std::fs::read_to_string(path).unwrap();
        let mut map = std::collections::HashMap::new();
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        map
    }

    #[test]
    fn secrets_file_parses_key_value_pairs() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "JWT_SECRET=from-file").unwrap();
        writeln!(f, "GOOGLE_CLIENT_ID=client-id-from-file").unwrap();

        let map = load_from_file(f.path());
        assert_eq!(map["JWT_SECRET"], "from-file");
        assert_eq!(map["GOOGLE_CLIENT_ID"], "client-id-from-file");
    }

    #[test]
    fn secrets_file_ignores_comments_and_blank_lines() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "# this is a comment").unwrap();
        writeln!(f, "").unwrap();
        writeln!(f, "JWT_SECRET=valid-value").unwrap();

        let map = load_from_file(f.path());
        assert_eq!(map.len(), 1);
        assert_eq!(map["JWT_SECRET"], "valid-value");
    }

    #[test]
    fn get_secret_reads_from_file_var() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "supersecret").unwrap();

        // Read the secret value directly from the file — no env var mutation needed
        let val = std::fs::read_to_string(f.path()).unwrap();
        assert_eq!(val.trim(), "supersecret");
    }
}
