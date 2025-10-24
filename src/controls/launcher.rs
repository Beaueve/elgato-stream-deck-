use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use tracing::{debug, info, warn};

use crate::config::LauncherButtonConfig;
use crate::hardware::{ButtonImage, DisplayPipeline};
use crate::system::desktop::DesktopEntry;
use crate::util::icons;

pub struct LauncherController {
    buttons: HashMap<u8, LauncherButton>,
}

impl LauncherController {
    pub fn new<H>(configs: &[LauncherButtonConfig], hardware: &H) -> Result<Option<Self>>
    where
        H: DisplayPipeline,
    {
        let mut buttons = HashMap::new();

        for entry in configs {
            match LauncherButton::from_config(entry) {
                Ok(button) => {
                    if let Some(previous) = buttons.insert(entry.button_index, button) {
                        warn!(
                            button_index = entry.button_index,
                            previous_entry = %previous.desktop_id,
                            "overriding previously configured launcher button"
                        );
                    }
                }
                Err(err) => {
                    warn!(
                        error = %err,
                        button_index = entry.button_index,
                        path = %entry.desktop_file.display(),
                        "skipping launcher button due to configuration error"
                    );
                }
            }
        }

        if buttons.is_empty() {
            return Ok(None);
        }

        for (index, button) in &buttons {
            if let Some(icon) = &button.icon {
                hardware
                    .update_button_icon(*index, Some(icon.clone()))
                    .with_context(|| format!("failed to set icon for launcher button {index}"))?;
            }
        }

        Ok(Some(Self { buttons }))
    }

    pub fn on_button_pressed(&self, index: u8) -> Result<bool> {
        if let Some(button) = self.buttons.get(&index) {
            button.activate()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[derive(Clone)]
struct LauncherButton {
    desktop_id: String,
    name: Option<String>,
    icon: Option<ButtonImage>,
    exec: Option<ExecSpec>,
    working_dir: Option<PathBuf>,
    terminal: bool,
    source_path: PathBuf,
}

impl LauncherButton {
    fn from_config(config: &LauncherButtonConfig) -> Result<Self> {
        let entry = DesktopEntry::from_path(&config.desktop_file)?;

        if let Some(entry_type) = entry.entry_type.as_deref() {
            if !entry_type.eq_ignore_ascii_case("application") {
                bail!("desktop entry type {entry_type:?} is not supported for launchers");
            }
        }

        let icon = resolve_icon(&entry)
            .transpose()?
            .map(|(id, image)| ButtonImage {
                id,
                image,
                tint: None,
            });

        let exec = parse_exec(&entry);

        Ok(Self {
            desktop_id: entry.desktop_id,
            name: entry.name,
            icon,
            exec,
            working_dir: entry.working_dir,
            terminal: entry.terminal,
            source_path: entry.source_path,
        })
    }

    fn activate(&self) -> Result<()> {
        info!(
            desktop_id = %self.desktop_id,
            app = self.name.as_deref().unwrap_or("Unnamed Application"),
            "activating launcher"
        );

        match try_gtk_launch(&self.desktop_id) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                debug!("gtk-launch not found on PATH; falling back to Exec command");
            }
            Err(err) => {
                warn!(
                    error = %err,
                    desktop_id = %self.desktop_id,
                    "gtk-launch failed; falling back to Exec command"
                );
            }
        }

        let exec = match &self.exec {
            Some(exec) => exec,
            None => {
                warn!(
                    desktop_id = %self.desktop_id,
                    path = %self.source_path.display(),
                    "launcher fallback unavailable: desktop entry lacks executable command"
                );
                bail!("no executable defined for {}", self.desktop_id);
            }
        };

        launch_exec(exec, self.working_dir.as_deref(), self.terminal).with_context(|| {
            format!(
                "failed to execute fallback command for desktop entry {}",
                self.desktop_id
            )
        })
    }
}

#[derive(Clone)]
struct ExecSpec {
    program: String,
    args: Vec<String>,
}

fn try_gtk_launch(desktop_id: &str) -> io::Result<()> {
    Command::new("gtk-launch")
        .arg(desktop_id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

fn launch_exec(spec: &ExecSpec, working_dir: Option<&Path>, terminal: bool) -> Result<()> {
    let mut command = Command::new(&spec.program);
    command.args(&spec.args);
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    if let Some(dir) = working_dir {
        command.current_dir(dir);
    }

    if terminal {
        warn!(
            command = %spec.program,
            "launcher desktop entry requests a terminal; executing command directly"
        );
    }

    command
        .spawn()
        .with_context(|| format!("failed to spawn {}", spec.program))?;
    Ok(())
}

fn parse_exec(entry: &DesktopEntry) -> Option<ExecSpec> {
    let command = entry.exec.as_ref()?;
    let tokens = split_exec(command);
    let mut processed = Vec::new();

    for token in tokens {
        if let Some(clean) = strip_field_codes(&token) {
            if !clean.is_empty() {
                processed.push(clean);
            }
        }
    }

    if processed.is_empty() {
        return None;
    }

    let program = processed.remove(0);
    Some(ExecSpec {
        program,
        args: processed,
    })
}

fn split_exec(command: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(ch) = chars.next() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            '\\' if !in_single => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if !current.is_empty() {
                    args.push(current);
                    current = String::new();
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    args
}

fn strip_field_codes(token: &str) -> Option<String> {
    let mut output = String::new();
    let mut chars = token.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            if let Some(next) = chars.next() {
                match next {
                    '%' => output.push('%'),
                    'f' | 'F' | 'u' | 'U' | 'd' | 'D' | 'n' | 'N' | 'v' | 'm' | 'k' | 'c' | 'i'
                    | 's' => return None,
                    other => {
                        debug!(placeholder = %other, "dropping unsupported Exec placeholder");
                        return None;
                    }
                }
            } else {
                output.push('%');
            }
        } else {
            output.push(ch);
        }
    }

    Some(output)
}

fn resolve_icon(
    entry: &DesktopEntry,
) -> Option<Result<(String, std::sync::Arc<image::RgbaImage>)>> {
    let icon = entry.icon.as_deref()?;
    let entry_dir = entry.source_path.parent();

    let path = Path::new(icon);
    if path.is_absolute() {
        return Some(load_icon_image(path, &entry.desktop_id));
    }

    if icon.contains('/') {
        if let Some(dir) = entry_dir {
            let joined = dir.join(icon);
            if joined.exists() {
                return Some(load_icon_image(&joined, &entry.desktop_id));
            }
            if let Some(found) = resolve_with_extensions(&joined) {
                return Some(load_icon_image(&found, &entry.desktop_id));
            }
        }
        let fallback = PathBuf::from(icon);
        if fallback.exists() {
            return Some(load_icon_image(&fallback, &entry.desktop_id));
        }
        if let Some(found) = resolve_with_extensions(&fallback) {
            return Some(load_icon_image(&found, &entry.desktop_id));
        }
    } else {
        if let Some(dir) = entry_dir {
            for candidate in icon_name_candidates(dir, icon) {
                if candidate.exists() {
                    return Some(load_icon_image(&candidate, &entry.desktop_id));
                }
            }
        }
    }

    for dir in icon_search_directories() {
        if let Some(found) = search_icon_in_dir(&dir, icon, 2) {
            return Some(load_icon_image(&found, &entry.desktop_id));
        }
    }

    None
}

fn load_icon_image(
    path: &Path,
    desktop_id: &str,
) -> Result<(String, std::sync::Arc<image::RgbaImage>)> {
    let id = format!(
        "launcher:{}:{}",
        desktop_id,
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("icon")
    );
    icons::load_icon(path).map(|image| (id, image))
}

fn resolve_with_extensions(base: &Path) -> Option<PathBuf> {
    for ext in ICON_EXTENSIONS {
        let candidate = base.with_extension(ext);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn icon_name_candidates(base: &Path, name: &str) -> Vec<PathBuf> {
    ICON_EXTENSIONS
        .iter()
        .map(|ext| base.join(format!("{name}.{ext}")))
        .collect()
}

const ICON_EXTENSIONS: &[&str] = &["svg", "png", "xpm", "jpg", "jpeg"];

fn icon_search_directories() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(xdg_data_home) = env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(&xdg_data_home).join("icons"));
    } else if let Some(home) = env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/icons"));
    }

    let data_dirs =
        env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':') {
        if dir.is_empty() {
            continue;
        }
        dirs.push(PathBuf::from(dir).join("icons"));
    }

    dirs.push(PathBuf::from("/usr/share/pixmaps"));
    dirs
}

fn search_icon_in_dir(base: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }

    for ext in ICON_EXTENSIONS {
        let candidate = base.join(format!("{name}.{ext}"));
        if candidate.exists() {
            return Some(candidate);
        }
    }

    let entries = fs::read_dir(base).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = search_icon_in_dir(&path, name, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use tempfile::tempdir;

    #[derive(Clone)]
    struct RecordingHardware {
        updates: Arc<std::sync::Mutex<Vec<(u8, Option<String>)>>>,
    }

    impl RecordingHardware {
        fn new() -> Self {
            Self {
                updates: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        fn updates(&self) -> Vec<(u8, Option<String>)> {
            self.updates.lock().unwrap().clone()
        }
    }

    impl DisplayPipeline for RecordingHardware {
        fn update_encoder(
            &self,
            _: crate::hardware::EncoderId,
            _: crate::hardware::EncoderDisplay,
        ) -> Result<()> {
            Ok(())
        }

        fn update_button_icon(&self, index: u8, icon: Option<ButtonImage>) -> Result<()> {
            self.updates
                .lock()
                .unwrap()
                .push((index, icon.map(|img| img.id)));
            Ok(())
        }
    }

    #[test]
    fn parses_exec_without_placeholders() {
        let entry = DesktopEntry {
            source_path: PathBuf::from("/tmp/app.desktop"),
            desktop_id: "app.desktop".into(),
            name: Some("App".into()),
            icon: None,
            exec: Some("env VAR=1 /usr/bin/app --flag".into()),
            try_exec: None,
            working_dir: None,
            terminal: false,
            startup_wm_class: None,
            entry_type: Some("Application".into()),
        };
        let spec = parse_exec(&entry).expect("exec should parse");
        assert_eq!(spec.program, "env");
        assert_eq!(spec.args, vec!["VAR=1", "/usr/bin/app", "--flag"]);
    }

    #[test]
    fn removes_field_codes_from_exec() {
        let entry = DesktopEntry {
            source_path: PathBuf::from("/tmp/app.desktop"),
            desktop_id: "app.desktop".into(),
            name: Some("App".into()),
            icon: None,
            exec: Some("\"/usr/bin/app\" %f --option=%u".into()),
            try_exec: None,
            working_dir: None,
            terminal: false,
            startup_wm_class: None,
            entry_type: Some("Application".into()),
        };
        let spec = parse_exec(&entry).expect("exec should parse");
        assert_eq!(spec.program, "/usr/bin/app");
        assert_eq!(spec.args, Vec::<String>::new());
    }

    #[test]
    fn controller_sets_icons_for_valid_entries() {
        let dir = tempdir().unwrap();
        let icon_path = dir.path().join("icon.svg");
        fs::write(
            &icon_path,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"></svg>"#,
        )
        .unwrap();

        let desktop_path = dir.path().join("app.desktop");
        fs::write(
            &desktop_path,
            format!(
                "[Desktop Entry]
Name=Sample App
Exec=/usr/bin/true
Icon={}
Type=Application
",
                icon_path.display()
            ),
        )
        .unwrap();

        let config = LauncherButtonConfig {
            button_index: 5,
            desktop_file: desktop_path.clone(),
        };

        let hardware = RecordingHardware::new();
        let controller = LauncherController::new(&[config], &hardware)
            .expect("launcher creation should succeed")
            .expect("launcher controller should be created");

        assert!(controller.buttons.contains_key(&5));
        let updates = hardware.updates();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].0, 5);
        assert!(updates[0].1.as_deref().unwrap().contains("launcher"));
    }
}
