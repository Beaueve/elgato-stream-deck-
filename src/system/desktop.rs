use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

#[derive(Debug, Clone)]
pub struct DesktopEntry {
    pub source_path: PathBuf,
    pub desktop_id: String,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub exec: Option<String>,
    pub try_exec: Option<String>,
    pub working_dir: Option<PathBuf>,
    pub terminal: bool,
    pub startup_wm_class: Option<String>,
    pub entry_type: Option<String>,
}

impl DesktopEntry {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read desktop entry at {}", path.display()))?;
        let fields = parse_desktop_entry(&contents)?;

        let desktop_id = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("desktop entry path {} lacks a file name", path.display()))?
            .to_string();

        let working_dir = fields
            .get("Path")
            .map(|value| resolve_relative_path(value, path));

        let terminal = fields
            .get("Terminal")
            .map(|value| value.eq_ignore_ascii_case("true") || value == "1")
            .unwrap_or(false);

        Ok(Self {
            source_path: path.to_path_buf(),
            desktop_id,
            name: fields.get("Name").cloned(),
            icon: fields.get("Icon").cloned(),
            exec: fields.get("Exec").cloned(),
            try_exec: fields.get("TryExec").cloned(),
            working_dir,
            terminal,
            startup_wm_class: fields.get("StartupWMClass").cloned(),
            entry_type: fields.get("Type").cloned(),
        })
    }
}

fn resolve_relative_path(value: &str, source: &Path) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        source
            .parent()
            .map(|parent| parent.join(value))
            .unwrap_or(path)
    }
}

fn parse_desktop_entry(contents: &str) -> Result<HashMap<String, String>> {
    let mut section = None;
    let mut fields = HashMap::new();

    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            let section_name = &line[1..line.len() - 1];
            section = Some(section_name.trim().to_string());
            continue;
        }

        if !matches!(section.as_deref(), Some("Desktop Entry")) {
            continue;
        }

        let mut parts = line.splitn(2, '=');
        let key = parts
            .next()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .ok_or_else(|| anyhow!("invalid desktop entry line: {line}"))?;
        let value = parts.next().map(str::trim).unwrap_or_default().to_string();
        fields.insert(key.to_string(), value);
    }

    if fields.is_empty() {
        Err(anyhow!(
            "desktop entry missing required [Desktop Entry] section"
        ))
    } else {
        Ok(fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    #[test]
    fn parses_basic_desktop_entry() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("example.desktop");
        fs::write(
            &path,
            "[Desktop Entry]
Name=Sample App
Exec=/usr/bin/sample --flag
Icon=/usr/share/icons/sample.svg
StartupWMClass=sample
Terminal=false
Type=Application
",
        )
        .unwrap();

        let entry = DesktopEntry::from_path(&path).unwrap();
        assert_eq!(entry.desktop_id, "example.desktop");
        assert_eq!(entry.name.as_deref(), Some("Sample App"));
        assert_eq!(entry.icon.as_deref(), Some("/usr/share/icons/sample.svg"));
        assert_eq!(entry.exec.as_deref(), Some("/usr/bin/sample --flag"));
        assert_eq!(entry.startup_wm_class.as_deref(), Some("sample"));
        assert!(!entry.terminal);
        assert_eq!(entry.entry_type.as_deref(), Some("Application"));
    }

    #[test]
    fn resolves_relative_working_directory() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("example.desktop");
        fs::write(
            &path,
            "[Desktop Entry]
Name=Sample App
Exec=sample
Path=tools
",
        )
        .unwrap();

        let entry = DesktopEntry::from_path(&path).unwrap();
        let expected = dir.path().join("tools");
        assert_eq!(entry.working_dir.as_deref(), Some(expected.as_path()));
    }
}
