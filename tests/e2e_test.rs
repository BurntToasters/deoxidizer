//! End-to-end tests: build realistic fixture trees and drive the real `deox`
//! binary. Every run writes a transcript (commands, exit codes, stdout,
//! stderr, and the asserted outcome) under `target/e2e-artifacts/` (or
//! `$DEOX_E2E_ARTIFACTS`) so a failure is reproducible from the artifact
//! alone.
//!
//! Failure modes covered (written before the fixes they guard):
//! 1. A symlink inside `target/` (Tauri AppImage `.DirIcon`, CMake `.so`
//!    links) blocks the whole clean.
//! 2. One unreadable folder under the projects dir aborts the entire scan.
//! 3. `clean --dry-run` reports bytes the real clean never frees.
//! 4. Projects with the same crate name are indistinguishable.
//! 5. Workspaces print one warning per member and are named after an
//!    arbitrary member; ignoring a member does not protect the shared target.
//! 6. `settings config --projects-dir .` stores a cwd-relative path.
//! 7. `--update` combined with a subcommand silently drops the subcommand.
//! 8. `inspect` on a nested path inside a project fails to find it.
//! 9. One project whose estimate fails aborts the whole clean.
//! 10. `full` silently deletes Tauri installer bundles; no way to keep them.
//! 11. A target with an active Cargo build lock is deleted mid-build.
//! 12. No machine-readable scan output.
//! 13. Nested `deps/deps` directories make `deps-only` report an error.
//! 14. A crate living in a directory literally named `target` is skipped.
//! 15. `clean --select` without a TTY must refuse, never fall through.
//! 16. Nearest workspace wins: an outer workspace's target is never
//!     attributed to an unrelated inner workspace.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn artifact_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("DEOX_E2E_ARTIFACTS") {
        let dir = PathBuf::from(dir);
        fs::create_dir_all(&dir).unwrap();
        return dir;
    }
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .map(|p| p.join("e2e-artifacts"))
        .unwrap_or_else(|| PathBuf::from("target/e2e-artifacts"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// One fixture world: isolated HOME, projects dir, and config file.
struct World {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    projects: PathBuf,
    config: PathBuf,
    log: fs::File,
}

impl World {
    fn new(name: &str, scope: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let projects = root.join("projects");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&projects).unwrap();
        let config = root.join("config.json");
        let json = serde_json::json!({
            "version": 2,
            "projects_dir": projects.to_string_lossy(),
            "scope": scope,
            "clean_behavior": "delete",
            "default_mode": "full",
            "min_size_mb": 0,
            "ignored_projects": [],
        });
        fs::write(&config, serde_json::to_string_pretty(&json).unwrap()).unwrap();
        let log = fs::File::create(artifact_dir().join(format!("{name}.log"))).unwrap();
        Self {
            _tmp: tmp,
            root,
            home,
            projects,
            config,
            log,
        }
    }

    fn set_config(&self, key: &str, value: serde_json::Value) {
        let mut json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&self.config).unwrap()).unwrap();
        json[key] = value;
        fs::write(&self.config, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    }

    fn run_in(&mut self, cwd: &Path, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_deox"));
        command
            .arg("--config")
            .arg(&self.config)
            .args(args)
            .current_dir(cwd)
            .env("NO_COLOR", "1")
            .env_remove("CLICOLOR_FORCE");
        #[cfg(windows)]
        command.env("USERPROFILE", &self.home);
        #[cfg(not(windows))]
        command.env("HOME", &self.home);
        let output = command.output().expect("run deox");
        writeln!(
            self.log,
            "$ deox --config <config> {}\n[exit {:?}]\n--- stdout ---\n{}--- stderr ---\n{}",
            args.join(" "),
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )
        .unwrap();
        output
    }

    fn run(&mut self, args: &[&str]) -> Output {
        let cwd = self.root.clone();
        self.run_in(&cwd, args)
    }

    fn note(&mut self, text: &str) {
        writeln!(self.log, "# {text}").unwrap();
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_ok(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed ({:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status.code(),
        stdout(output),
        stderr(output)
    );
}

fn crate_at(dir: &Path, name: &str, deps: &str) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{deps}\n"),
    )
    .unwrap();
    fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
}

fn file(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![0u8; bytes]).unwrap();
}

/// Parse `freed N` / `Would free N` byte counts printed by `--json`-free
/// output by re-running with `--json` instead; keeps assertions exact.
fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON ({error}):\n{}\n--- stderr ---\n{}",
            stdout(output),
            stderr(output)
        )
    })
}

#[cfg(unix)]
#[test]
fn e2e_01_symlink_inside_target_does_not_block_clean() {
    let mut w = World::new("01-symlink-inside-target", "tauri-only");
    let app = w.projects.join("appimg/src-tauri");
    crate_at(&app, "appimg", "tauri = \"2\"");
    file(&app.join("target/debug/deps/libx.rlib"), 4096);
    let appdir = app.join("target/release/bundle/appimage/appimg.AppDir");
    file(&appdir.join("appimg.png"), 100);
    std::os::unix::fs::symlink("appimg.png", appdir.join(".DirIcon")).unwrap();
    // A link that points outside the project must be removed as a link,
    // never followed.
    let sentinel = w.root.join("outside-sentinel");
    fs::write(&sentinel, b"must survive").unwrap();
    std::os::unix::fs::symlink(&sentinel, app.join("target/debug/escape")).unwrap();

    let out = w.run(&["clean", "-y"]);
    assert_ok(&out, "clean with symlinks inside target");
    assert!(!app.join("target").exists(), "target must be removed");
    assert_eq!(fs::read(&sentinel).unwrap(), b"must survive");
    w.note("PASS: interior symlinks removed as links; outside file intact");
}

#[cfg(unix)]
#[test]
fn e2e_02_unreadable_folder_does_not_abort_scan() {
    use std::os::unix::fs::PermissionsExt;
    let mut w = World::new("02-unreadable-folder", "tauri-only");
    let app = w.projects.join("good/src-tauri");
    crate_at(&app, "good", "tauri = \"2\"");
    file(&app.join("target/debug/x"), 1000);
    let locked = w.projects.join("docker-data");
    fs::create_dir_all(locked.join("inner")).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&locked).is_ok() {
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        w.note("SKIP: running with privileges that bypass permissions");
        return;
    }
    let out = w.run(&["scan"]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_ok(&out, "scan with an unreadable folder");
    assert!(stdout(&out).contains("good"), "good project still listed");
    assert!(
        stderr(&out).contains("docker-data"),
        "skipped folder is reported"
    );
    w.note("PASS: unreadable folder skipped with a warning");
}

#[test]
fn e2e_03_dry_run_matches_actual_clean() {
    let mut w = World::new("03-dry-run-accuracy", "tauri-and-rust");
    let a = w.projects.join("a");
    crate_at(&a, "a", "");
    // Files with `debug`/`deps`/`incremental` deep in release paths are not
    // removed by the selective modes and must not be counted.
    file(
        &a.join("target/release/build/openssl-sys-1/out/debug/big.a"),
        50_000,
    );
    file(&a.join("target/release/build/x-1/out/deps/d.o"), 30_000);
    file(
        &a.join("target/release/build/x-1/out/lib/incremental/i"),
        20_000,
    );
    file(&a.join("target/debug/deps/x.rlib"), 10_000);
    file(&a.join("target/debug/incremental/s/q"), 7_000);
    file(
        &a.join("target/x86_64-unknown-linux-gnu/debug/deps/t.rlib"),
        3_000,
    );

    for (mode, expected) in [
        ("debug-only", 20_000u64),
        ("deps-only", 13_000),
        ("incremental-only", 7_000),
    ] {
        let dry = w.run(&["clean", "-m", mode, "--dry-run", "--json"]);
        assert_ok(&dry, "dry run");
        let predicted = json(&dry)["bytes"].as_u64().unwrap();
        assert_eq!(predicted, expected, "dry-run bytes for {mode}");
    }
    let dry = w.run(&["clean", "-m", "debug-only", "--dry-run", "--json"]);
    let predicted = json(&dry)["bytes"].as_u64().unwrap();
    let real = w.run(&["clean", "-m", "debug-only", "-y", "--json"]);
    assert_ok(&real, "real clean");
    assert_eq!(json(&real)["bytes"].as_u64().unwrap(), predicted);
    w.note("PASS: dry-run bytes equal freed bytes");
}

#[test]
fn e2e_04_same_name_projects_are_distinguishable() {
    let mut w = World::new("04-duplicate-names", "tauri-only");
    for dir in ["one", "two"] {
        let app = w.projects.join(dir).join("src-tauri");
        crate_at(&app, "app", "tauri = \"2\"");
        file(&app.join("target/debug/x"), 1000);
    }
    let out = w.run(&["scan"]);
    assert_ok(&out, "scan");
    let text = stdout(&out);
    assert!(
        text.contains("one") && text.contains("two"),
        "paths shown: {text}"
    );
    let out = w.run(&["scan", "--json"]);
    let projects = json(&out)["projects"].as_array().unwrap().clone();
    assert_eq!(projects.len(), 2);
    assert_ne!(projects[0]["path"], projects[1]["path"]);
    w.note("PASS: duplicate names disambiguated by path");
}

#[test]
fn e2e_05_workspace_is_one_quiet_named_project() {
    let mut w = World::new("05-workspace", "tauri-and-rust");
    let ws = w.projects.join("myws");
    fs::create_dir_all(&ws).unwrap();
    fs::write(
        ws.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\n",
    )
    .unwrap();
    for member in ["a", "b", "c"] {
        crate_at(&ws.join("crates").join(member), member, "");
    }
    file(&ws.join("target/debug/x"), 1000);

    let out = w.run(&["scan", "--json"]);
    assert_ok(&out, "scan");
    assert!(
        !stderr(&out).contains("duplicate target"),
        "no per-member warnings: {}",
        stderr(&out)
    );
    let projects = json(&out)["projects"].as_array().unwrap().clone();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["name"], "myws");

    // Ignoring one member protects the shared target it builds into.
    w.set_config("ignored_projects", serde_json::json!(["b"]));
    let out = w.run(&["clean", "-y"]);
    assert_ok(&out, "clean with ignored member");
    assert!(
        ws.join("target").exists(),
        "shared target must be protected"
    );
    w.note("PASS: workspace grouped, named, quiet, and ignore-protected");
}

#[test]
fn e2e_06_settings_store_absolute_projects_dir() {
    let mut w = World::new("06-relative-projects-dir", "tauri-only");
    let cwd = w.projects.clone();
    let out = w.run_in(&cwd, &["settings", "config", "--projects-dir", "."]);
    assert_ok(&out, "settings config");
    let stored: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&w.config).unwrap()).unwrap();
    let stored = stored["projects_dir"].as_str().unwrap().to_string();
    assert!(Path::new(&stored).is_absolute(), "stored {stored}");
    assert_eq!(dunce::canonicalize(&stored).unwrap(), w.projects);
    w.note("PASS: relative projects dir stored as absolute");
}

#[test]
fn e2e_07_update_with_subcommand_is_rejected() {
    let mut w = World::new("07-update-conflict", "tauri-only");
    let out = w.run(&["--update", "scan"]);
    assert_eq!(out.status.code(), Some(2), "usage error expected");
    assert!(!stdout(&out).contains("Checking for updates"));
    w.note("PASS: --update with a subcommand is a usage error, no network");
}

#[test]
fn e2e_08_inspect_nested_path_finds_project() {
    let mut w = World::new("08-inspect-nested", "tauri-only");
    let app = w.projects.join("proj/src-tauri");
    crate_at(&app, "proj", "tauri = \"2\"");
    file(&app.join("target/debug/x"), 1000);
    let nested = app.join("src/bin/tool.rs");
    file(&nested, 10);
    let out = w.run(&["inspect", nested.to_str().unwrap()]);
    assert_ok(&out, "inspect nested file");
    assert!(stdout(&out).contains("proj"));
    let empty = w.projects.join("not-a-project");
    fs::create_dir_all(&empty).unwrap();
    let out = w.run(&["inspect", empty.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    w.note("PASS: nested inspect resolves the enclosing project");
}

#[cfg(unix)]
#[test]
fn e2e_09_one_failing_project_does_not_abort_clean() {
    use std::os::unix::fs::PermissionsExt;
    let mut w = World::new("09-partial-failure", "tauri-and-rust");
    let good = w.projects.join("good");
    crate_at(&good, "good", "");
    file(&good.join("target/debug/x"), 1000);
    let bad = w.projects.join("bad");
    crate_at(&bad, "bad", "");
    file(&bad.join("target/debug/sub/x"), 1000);
    let sub = bad.join("target/debug/sub");
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&sub).is_ok() {
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
        w.note("SKIP: running with privileges that bypass permissions");
        return;
    }
    let out = w.run(&["clean", "-y"]);
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(out.status.code(), Some(1), "errors reported via exit code");
    assert!(!good.join("target").exists(), "good project still cleaned");
    w.note("PASS: failing project reported, others cleaned");
}

#[test]
fn e2e_10_tauri_bundles_warned_and_keepable() {
    let mut w = World::new("10-keep-bundles", "tauri-only");
    let app = w.projects.join("app/src-tauri");
    crate_at(&app, "app", "tauri = \"2\"");
    file(&app.join("target/debug/x"), 1000);
    file(&app.join("target/release/app"), 1000);
    file(&app.join("target/release/bundle/dmg/app.dmg"), 500);
    file(
        &app.join("target/aarch64-apple-darwin/release/bundle/macos/app.zip"),
        300,
    );

    let out = w.run(&["clean", "--dry-run"]);
    assert_ok(&out, "dry run");
    assert!(
        stdout(&out).to_lowercase().contains("bundle"),
        "bundle warning shown"
    );
    let out = w.run(&["clean", "-y", "--keep-bundles"]);
    assert_ok(&out, "clean --keep-bundles");
    assert!(app.join("target/release/bundle/dmg/app.dmg").exists());
    assert!(app
        .join("target/aarch64-apple-darwin/release/bundle/macos/app.zip")
        .exists());
    assert!(!app.join("target/debug").exists());
    assert!(!app.join("target/release/app").exists());
    w.note("PASS: bundles warned about and preserved with --keep-bundles");
}

#[test]
fn e2e_11_active_build_lock_skips_project() {
    let mut w = World::new("11-build-lock", "tauri-and-rust");
    let busy = w.projects.join("busy");
    crate_at(&busy, "busy", "");
    file(&busy.join("target/debug/x"), 1000);
    let lock_path = busy.join("target/debug/.cargo-lock");
    fs::write(&lock_path, b"").unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    lock.lock().unwrap();
    let idle = w.projects.join("idle");
    crate_at(&idle, "idle", "");
    file(&idle.join("target/debug/x"), 1000);

    let out = w.run(&["clean", "-y"]);
    lock.unlock().unwrap();
    assert_ok(&out, "clean while one build is active");
    assert!(
        busy.join("target/debug/x").exists(),
        "busy target untouched"
    );
    assert!(!idle.join("target").exists(), "idle target cleaned");
    assert!(stdout(&out).contains("build in progress"));
    w.note("PASS: locked target skipped");
}

#[test]
fn e2e_12_scan_json_is_machine_readable() {
    let mut w = World::new("12-json", "tauri-only");
    let app = w.projects.join("j/src-tauri");
    crate_at(&app, "j", "tauri = \"2\"");
    file(&app.join("target/debug/x"), 1234);
    let out = w.run(&["scan", "--json"]);
    assert_ok(&out, "scan --json");
    let value = json(&out);
    let project = &value["projects"][0];
    assert_eq!(project["name"], "j");
    assert_eq!(project["kind"], "tauri");
    assert_eq!(project["artifact_size"], 1234);
    assert_eq!(value["total_bytes"], 1234);
    w.note("PASS: JSON scan output");
}

#[test]
fn e2e_13_nested_deps_dirs_clean_without_error() {
    let mut w = World::new("13-nested-deps", "tauri-and-rust");
    let a = w.projects.join("n");
    crate_at(&a, "n", "");
    file(&a.join("target/debug/deps/deps/x"), 100);
    file(&a.join("target/debug/deps/y"), 100);
    let out = w.run(&["clean", "-m", "deps-only", "-y"]);
    assert_ok(&out, "deps-only with nested deps");
    assert!(!a.join("target/debug/deps").exists());
    w.note("PASS: nested matches deduplicated");
}

#[test]
fn e2e_14_crate_in_directory_named_target_is_found() {
    let mut w = World::new("14-target-named-crate", "tauri-and-rust");
    let tools = w.projects.join("tools/target");
    crate_at(&tools, "target-tool", "");
    file(&tools.join("target/debug/x"), 100);
    let out = w.run(&["scan", "--json"]);
    assert_ok(&out, "scan");
    let names: Vec<String> = json(&out)["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["target-tool".to_string()]);
    w.note("PASS: non-artifact `target` directory traversed");
}

#[test]
fn e2e_15_select_without_tty_refuses() {
    let mut w = World::new("15-select-no-tty", "tauri-and-rust");
    let a = w.projects.join("s");
    crate_at(&a, "s", "");
    file(&a.join("target/debug/x"), 100);
    let out = w.run(&["clean", "--select", "-y"]);
    assert_eq!(out.status.code(), Some(1), "selection needs a terminal");
    assert!(a.join("target/debug/x").exists());
    w.note("PASS: --select without TTY refuses and deletes nothing");
}

#[test]
fn e2e_16_nearest_workspace_owns_member() {
    let mut w = World::new("16-nearest-workspace", "tauri-and-rust");
    let outer = w.projects.join("outer");
    fs::create_dir_all(&outer).unwrap();
    fs::write(
        outer.join("Cargo.toml"),
        "[workspace]\nmembers = []\nexclude = [\"vendor/inner\"]\n",
    )
    .unwrap();
    file(&outer.join("target/debug/x"), 100);
    let inner = outer.join("vendor/inner");
    fs::create_dir_all(&inner).unwrap();
    fs::write(inner.join("Cargo.toml"), "[workspace]\nmembers = [\"m\"]\n").unwrap();
    crate_at(&inner.join("m"), "m", "");
    let out = w.run(&["scan", "--json"]);
    assert_ok(&out, "scan");
    let projects = json(&out)["projects"].as_array().unwrap().clone();
    assert_eq!(projects.len(), 1, "{projects:?}");
    assert_eq!(projects[0]["name"], "outer");
    w.note("PASS: inner workspace without target does not claim outer target");
}
