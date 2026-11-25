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
//! 17. A `target/` without Cargo's marker (CACHEDIR.TAG or .rustc_info.json)
//!     is not a build directory: never scanned, never cleaned.
//! 18. A crate nested in a plain folder named `target` is found and cleaned.
//! 19. A project directory with non-UTF-8 bytes crashes `clean -y` and
//!     `scan --json` (paths were serialized through `json!`).
//! 20. Package names with ESC/BEL reach the terminal unescaped.
//! 21. Scan and clean JSON carry no `schema_version`.
//! 22. A failed confirmation prompt does not say how to skip it.
//! 23. Explicit paths with significant whitespace and relative stored roots
//!     can select a different tree than the user intended.
//! 24. A failed config backup does not stop setup from overwriting the config.
//! 25. Workspace ignores include hidden and stale-local-target members.
//! 26. Workspace excludes do not claim package ownership.
//! 27. Symlinked ancestor manifests cannot establish workspace ownership.
//! 28. Indeterminate Cargo lock entries fail closed, including during dry-run.
//! 29. Filesystem errors are sanitized at terminal sinks while JSON names stay raw.
//! 30. Cargo target markers must be regular files, not links.
//! 31. Windows directory-link leaves are removed without following them.
//! 32. Trash must fail closed when the backend cannot bind the request to a verified directory identity.
//! 33. Unresolved declared workspace members cannot bypass ignore protection.
//! 34. Linux Trash stages a verified entry and preserves its original restore path.
//! 35. Linux Trash failures restore the original entry without leaving staged data behind.
//! 36. Windows directory-link leaves are removed without following them.

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
        #[cfg(target_os = "linux")]
        command.env("XDG_DATA_HOME", self.home.join(".local/share"));
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

/// The `CACHEDIR.TAG` Cargo writes into every target directory it builds.
const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by cargo.\n";

/// Create `target` the way Cargo does, with its `CACHEDIR.TAG` marker.
fn mark_cargo_target(target: &Path) {
    fs::create_dir_all(target).unwrap();
    fs::write(target.join("CACHEDIR.TAG"), CACHEDIR_TAG).unwrap();
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
    mark_cargo_target(&app.join("target"));
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
    mark_cargo_target(&app.join("target"));
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
    mark_cargo_target(&a.join("target"));
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
        mark_cargo_target(&app.join("target"));
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
    mark_cargo_target(&ws.join("target"));
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
    mark_cargo_target(&app.join("target"));
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
    mark_cargo_target(&good.join("target"));
    file(&good.join("target/debug/x"), 1000);
    let bad = w.projects.join("bad");
    crate_at(&bad, "bad", "");
    mark_cargo_target(&bad.join("target"));
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
    mark_cargo_target(&app.join("target"));
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
    mark_cargo_target(&busy.join("target"));
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
    mark_cargo_target(&idle.join("target"));
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
    mark_cargo_target(&app.join("target"));
    file(&app.join("target/debug/x"), 1234);
    let out = w.run(&["scan", "--json"]);
    assert_ok(&out, "scan --json");
    let value = json(&out);
    let project = &value["projects"][0];
    assert_eq!(project["name"], "j");
    assert_eq!(project["kind"], "tauri");
    // The CACHEDIR.TAG marker is part of the target and counts toward size.
    let expected = 1234 + CACHEDIR_TAG.len() as u64;
    assert_eq!(project["artifact_size"], expected);
    assert_eq!(value["total_bytes"], expected);
    w.note("PASS: JSON scan output");
}

#[test]
fn e2e_13_nested_deps_dirs_clean_without_error() {
    let mut w = World::new("13-nested-deps", "tauri-and-rust");
    let a = w.projects.join("n");
    crate_at(&a, "n", "");
    mark_cargo_target(&a.join("target"));
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
    mark_cargo_target(&tools.join("target"));
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
    mark_cargo_target(&a.join("target"));
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
    mark_cargo_target(&outer.join("target"));
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

#[test]
fn e2e_17_unmarked_target_is_never_scanned_or_cleaned() {
    let mut w = World::new("17-unmarked-target", "tauri-and-rust");
    // Reproduces the audit bug: a crate whose target/ Cargo never created.
    let ghost = w.projects.join("ghost");
    crate_at(&ghost, "ghost", "");
    file(&ghost.join("target/specs/x86_64-os.json"), 300);
    file(&ghost.join("target/README.md"), 120);
    // A marked sibling project is still found and cleaned.
    let real = w.projects.join("real");
    crate_at(&real, "real", "");
    mark_cargo_target(&real.join("target"));
    file(&real.join("target/debug/x"), 1000);

    let scan = w.run(&["scan", "--json"]);
    assert_ok(&scan, "scan");
    let names: Vec<String> = json(&scan)["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["real".to_string()], "{}", stdout(&scan));
    assert!(
        stderr(&scan).contains("not a Cargo build directory"),
        "unmarked target must be reported:\n{}",
        stderr(&scan)
    );

    let clean = w.run(&["clean", "-y"]);
    assert_ok(&clean, "clean -y");
    assert!(ghost.join("target/specs/x86_64-os.json").exists());
    assert!(ghost.join("target/README.md").exists());
    assert!(!real.join("target").exists(), "marked target cleaned");
    w.note("PASS: unmarked target skipped by scan and clean; marked sibling cleaned");
}

#[test]
fn e2e_18_crate_nested_in_plain_target_folder_is_found() {
    let mut w = World::new("18-nested-in-plain-target", "tauri-and-rust");
    // `target/` here is a plain folder (no Cargo.toml, no marker); the crate
    // inside it must still be discovered and its own marked target cleaned.
    let app = w.projects.join("holder/target/app");
    crate_at(&app, "app", "");
    mark_cargo_target(&app.join("target"));
    file(&app.join("target/debug/x"), 1000);

    let scan = w.run(&["scan", "--json"]);
    assert_ok(&scan, "scan");
    let names: Vec<String> = json(&scan)["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["app".to_string()], "{}", stdout(&scan));

    let clean = w.run(&["clean", "-y"]);
    assert_ok(&clean, "clean -y");
    assert!(!app.join("target").exists(), "nested app target cleaned");
    assert!(app.join("Cargo.toml").exists(), "source tree untouched");
    w.note("PASS: crate in plain target/ folder found and cleaned");
}

#[cfg(unix)]
#[test]
fn e2e_19_non_utf8_project_dir_cleans_and_serializes() {
    use std::os::unix::ffi::OsStrExt;
    let mut w = World::new("19-non-utf8-path", "tauri-and-rust");
    let plain = w.projects.join("plain");
    crate_at(&plain, "plain", "");
    mark_cargo_target(&plain.join("target"));
    file(&plain.join("target/debug/x"), 1000);
    // Byte 0xE9 is Latin-1 `é`: not valid UTF-8, but a legal Unix file name.
    let latin = w.projects.join(std::ffi::OsStr::from_bytes(b"caf\xe9"));
    // APFS (macOS) rejects non-UTF-8 names with EILSEQ; such paths cannot
    // exist there, so the crash this guards against cannot happen.
    if let Err(error) = fs::create_dir(&latin) {
        w.note(&format!(
            "SKIP: filesystem rejects non-UTF-8 directory names ({error})"
        ));
        return;
    }
    crate_at(&latin, "latin", "");
    mark_cargo_target(&latin.join("target"));
    file(&latin.join("target/debug/x"), 1000);

    let scan = w.run(&["scan", "--json"]);
    assert_ok(&scan, "scan --json with non-UTF-8 path");
    let value = json(&scan);
    assert_eq!(value["projects"].as_array().unwrap().len(), 2);

    let out = w.run(&["clean", "-y"]);
    assert_ok(&out, "clean -y with non-UTF-8 path");
    assert!(!plain.join("target").exists(), "plain cleaned");
    assert!(!latin.join("target").exists(), "non-UTF-8 project cleaned");
    w.note("PASS: non-UTF-8 project path cleaned in human mode and scanned as JSON");
}

#[test]
fn e2e_20_terminal_escapes_in_names_are_neutralized() {
    let mut w = World::new("20-terminal-escapes", "tauri-only");
    let app = w.projects.join("evil/src-tauri");
    fs::create_dir_all(app.join("src")).unwrap();
    // TOML `\u` escapes decode to a real ESC and BEL in the package name.
    fs::write(
        app.join("Cargo.toml"),
        "[package]\nname = \"evil\\u001b]0;pwned\\u0007\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ntauri = \"2\"\n",
    )
    .unwrap();
    fs::write(app.join("src/main.rs"), "fn main() {}\n").unwrap();
    mark_cargo_target(&app.join("target"));
    file(&app.join("target/debug/x"), 1000);

    let human = w.run(&["scan"]);
    assert_ok(&human, "scan");
    for (label, bytes) in [("stdout", &human.stdout), ("stderr", &human.stderr)] {
        assert!(!bytes.contains(&0x1b), "ESC byte in {label}");
        assert!(!bytes.contains(&0x07), "BEL byte in {label}");
    }
    assert!(stdout(&human).contains("pwned"), "name still shown");

    let dry = w.run(&["clean", "--dry-run"]);
    assert_ok(&dry, "clean --dry-run");
    assert!(!dry.stdout.contains(&0x1b), "ESC byte in clean output");
    assert!(!dry.stdout.contains(&0x07), "BEL byte in clean output");

    // JSON stays faithful: serde escapes control characters.
    let out = w.run(&["scan", "--json"]);
    assert_ok(&out, "scan --json");
    assert_eq!(
        json(&out)["projects"][0]["name"].as_str().unwrap(),
        "evil\u{1b}]0;pwned\u{7}"
    );
    w.note("PASS: ESC/BEL stripped from human output; JSON keeps exact name");
}

#[test]
fn e2e_21_json_outputs_carry_schema_version() {
    let mut w = World::new("21-schema-version", "tauri-only");
    let empty_scan = w.run(&["scan", "--json"]);
    assert_ok(&empty_scan, "empty scan --json");
    assert_eq!(json(&empty_scan)["schema_version"], 1);
    let empty_clean = w.run(&["clean", "--json"]);
    assert_ok(&empty_clean, "empty clean --json");
    assert_eq!(json(&empty_clean)["schema_version"], 1);
    let empty_dry = w.run(&["clean", "--dry-run", "--json"]);
    assert_ok(&empty_dry, "empty dry-run --json");
    assert_eq!(json(&empty_dry)["schema_version"], 1);

    let app = w.projects.join("sv/src-tauri");
    crate_at(&app, "sv", "tauri = \"2\"");
    mark_cargo_target(&app.join("target"));
    file(&app.join("target/debug/x"), 1000);
    let scan = w.run(&["scan", "--json"]);
    assert_eq!(json(&scan)["schema_version"], 1);
    let dry = w.run(&["clean", "--dry-run", "--json"]);
    assert_eq!(json(&dry)["schema_version"], 1);
    let real = w.run(&["clean", "-y", "--json"]);
    assert_ok(&real, "clean -y --json");
    assert_eq!(json(&real)["schema_version"], 1);
    assert_eq!(json(&real)["dry_run"], false);
    w.note("PASS: schema_version 1 on scan and clean JSON, including empty results");
}

#[cfg(unix)]
#[test]
fn e2e_22_confirmation_failure_names_the_yes_flag() {
    let mut w = World::new("22-confirm-failure", "tauri-only");
    let app = w.projects.join("cf/src-tauri");
    crate_at(&app, "cf", "tauri = \"2\"");
    mark_cargo_target(&app.join("target"));
    file(&app.join("target/debug/x"), 1000);
    // No terminal and no --yes: the prompt cannot be answered.
    let out = w.run(&["clean"]);
    assert_eq!(out.status.code(), Some(1), "prompt failure exits 1");
    let err = stderr(&out);
    assert!(
        err.contains("Confirmation failed:")
            && err.contains("Use --yes to clean without a prompt."),
        "stderr: {err}"
    );
    assert!(app.join("target/debug/x").exists(), "nothing deleted");
    w.note("PASS: prompt failure points at --yes and deletes nothing");
}

#[test]
fn e2e_23_keep_bundles_target_stays_a_cargo_build_dir() {
    // Failure modes: --keep-bundles deletes CACHEDIR.TAG, so the kept bundles
    // become an "unverified" target that is warned about on every scan and
    // can never be cleaned afterwards.
    let mut w = World::new("23-keep-bundles-keeps-marker", "tauri-only");
    let app = w.projects.join("app/src-tauri");
    crate_at(&app, "app", "tauri = \"2\"");
    mark_cargo_target(&app.join("target"));
    file(&app.join("target/debug/x"), 1000);
    file(&app.join("target/release/bundle/dmg/app.dmg"), 500);

    let out = w.run(&["clean", "-y", "--keep-bundles"]);
    assert_ok(&out, "clean --keep-bundles");
    assert!(app.join("target/release/bundle/dmg/app.dmg").exists());
    assert!(app.join("target/CACHEDIR.TAG").exists(), "marker kept");

    let out = w.run(&["scan"]);
    assert_ok(&out, "scan after keep-bundles");
    assert!(
        !stderr(&out).contains("not a Cargo build directory"),
        "no unverified-target warning: {}",
        stderr(&out)
    );
    let out = w.run(&["clean", "-y"]);
    assert_ok(&out, "full clean of kept bundles");
    assert!(
        !app.join("target").exists(),
        "bundles removed by full clean"
    );
    w.note("PASS: --keep-bundles keeps the Cargo marker; bundles stay cleanable");
}

#[test]
fn e2e_24_explicit_paths_preserve_whitespace_and_relative_configs_fail_closed() {
    let mut w = World::new("24-path-identity", "tauri-and-rust");
    let spaced = w.root.join(" projects ");
    fs::create_dir_all(&spaced).unwrap();
    let app = spaced.join("space-app");
    crate_at(&app, "space-app", "");
    mark_cargo_target(&app.join("target"));
    file(&app.join("target/debug/x"), 100);

    let path_arg = spaced.to_str().unwrap();
    let dry = w.run(&["clean", "--dry-run", "--json", "--path", path_arg]);
    assert_ok(&dry, "clean with a whitespace-sensitive --path");
    assert_eq!(json(&dry)["projects"].as_array().unwrap().len(), 1);

    let settings = w.run(&["settings", "config", "--projects-dir", path_arg]);
    assert_ok(
        &settings,
        "settings config with a whitespace-sensitive path",
    );
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(&w.config).unwrap()).unwrap();
    assert_eq!(saved["projects_dir"], path_arg);

    w.set_config("projects_dir", serde_json::json!("./projects"));
    let rejected = w.run(&["clean", "-y"]);
    assert_eq!(rejected.status.code(), Some(1));
    assert!(stderr(&rejected).contains("projects_dir must be absolute"));
    assert!(app.join("target/debug/x").exists());
    w.note("PASS: explicit whitespace preserved; relative stored root rejected before clean");
}

#[cfg(unix)]
#[test]
fn e2e_25_setup_backup_collision_preserves_backup_and_original_config() {
    let mut w = World::new("25-setup-backup-collision", "tauri-only");
    let original_config = fs::read(&w.config).unwrap();
    let sentinel = w.root.join("backup-sentinel");
    fs::write(&sentinel, b"backup destination must survive").unwrap();
    let mut backup_name = w.config.as_os_str().to_os_string();
    backup_name.push(".bak");
    let backup = PathBuf::from(backup_name);
    std::os::unix::fs::symlink(&sentinel, &backup).unwrap();

    let out = w.run(&["setup", "--default", "--yes"]);
    assert_eq!(out.status.code(), Some(1), "backup failure must stop setup");
    assert_eq!(fs::read(&w.config).unwrap(), original_config);
    assert_eq!(
        fs::read(&sentinel).unwrap(),
        b"backup destination must survive"
    );
    assert!(fs::symlink_metadata(&backup)
        .unwrap()
        .file_type()
        .is_symlink());
    w.note("PASS: backup collision stops setup without following or overwriting destination");
}

#[test]
fn e2e_26_hidden_workspace_member_and_stale_local_target_are_ignored() {
    let mut w = World::new("26-hidden-workspace-member", "tauri-and-rust");
    let workspace = w.projects.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(
        workspace.join("Cargo.toml"),
        "[workspace]\nmembers = [\".hidden-member\", \"visible\"]\n",
    )
    .unwrap();
    crate_at(&workspace.join(".hidden-member"), "protected-hidden", "");
    crate_at(&workspace.join("visible"), "visible", "");
    // This marked local target can make the hidden package appear to own a
    // separate target even though the workspace target also contains builds.
    mark_cargo_target(&workspace.join(".hidden-member/target"));
    file(&workspace.join(".hidden-member/target/debug/local"), 100);
    mark_cargo_target(&workspace.join("target"));
    file(&workspace.join("target/debug/shared"), 200);
    w.set_config("ignored_projects", serde_json::json!(["protected-hidden"]));

    let out = w.run(&["clean", "-y"]);
    assert_ok(&out, "clean with ignored hidden workspace member");
    assert!(workspace.join("target/debug/shared").exists());
    assert!(workspace.join(".hidden-member/target/debug/local").exists());
    w.note("PASS: hidden member protects shared target and its marked local target");
}

#[test]
fn e2e_27_workspace_excludes_do_not_claim_package_ownership() {
    let mut w = World::new("27-workspace-excludes", "tauri-and-rust");
    let workspace = w.projects.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(
        workspace.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/excluded\"]\n",
    )
    .unwrap();
    crate_at(&workspace.join("crates/included"), "included", "");
    crate_at(&workspace.join("crates/excluded"), "excluded", "");
    mark_cargo_target(&workspace.join("target"));
    file(&workspace.join("target/debug/shared"), 200);
    w.set_config("ignored_projects", serde_json::json!(["excluded"]));

    let out = w.run(&["clean", "-y"]);
    assert_ok(&out, "clean with ignored excluded package");
    assert!(!workspace.join("target").exists());
    w.note("PASS: excluded package does not protect the workspace target");
}

#[cfg(unix)]
#[test]
fn e2e_28_symlinked_ancestor_manifest_cannot_establish_workspace() {
    let mut w = World::new("28-symlinked-manifest", "tauri-and-rust");
    let external = w.root.join("external");
    fs::create_dir_all(&external).unwrap();
    fs::write(
        external.join("Cargo.toml"),
        "[workspace]\nmembers = [\"member\"]\n",
    )
    .unwrap();
    let project = w.projects.join("linked-workspace");
    fs::create_dir_all(&project).unwrap();
    std::os::unix::fs::symlink(external.join("Cargo.toml"), project.join("Cargo.toml")).unwrap();
    crate_at(&project.join("member"), "member", "");
    mark_cargo_target(&project.join("target"));
    file(&project.join("target/debug/x"), 100);

    let scan = w.run(&["scan", "--json"]);
    assert_ok(&scan, "scan with symlinked ancestor manifest");
    assert!(json(&scan)["projects"].as_array().unwrap().is_empty());
    let clean = w.run(&["clean", "-y"]);
    assert_ok(&clean, "clean with symlinked ancestor manifest");
    assert!(project.join("target/debug/x").exists());
    w.note("PASS: symlinked Cargo.toml cannot establish clean eligibility");
}

#[test]
fn e2e_29_dry_run_and_clean_fail_closed_on_indeterminate_cargo_lock() {
    let mut w = World::new("29-cargo-lock-parity", "tauri-and-rust");
    let app = w.projects.join("locked");
    crate_at(&app, "locked", "");
    mark_cargo_target(&app.join("target"));
    file(&app.join("target/debug/x"), 100);
    let lock_path = app.join("target/debug/.cargo-lock");
    // Cargo expects a regular lock file. A directory at the same name is an
    // indeterminate state and must not be mistaken for an idle build.
    fs::create_dir(&lock_path).unwrap();

    let dry = w.run(&["clean", "--dry-run", "--json"]);
    assert_eq!(
        dry.status.code(),
        Some(1),
        "indeterminate lock state is an error"
    );
    let dry_json = json(&dry);
    assert_eq!(dry_json["bytes"], 0);
    assert_eq!(dry_json["projects"][0]["result"], "error");
    assert!(app.join("target/debug/x").exists());

    let clean = w.run(&["clean", "-y", "--json"]);
    assert_eq!(clean.status.code(), Some(1));
    assert_eq!(json(&clean)["projects"][0]["result"], "error");
    assert!(app.join("target/debug/x").exists());
    w.note("PASS: unknown Cargo lock state is never estimated or cleaned");
}

#[test]
fn e2e_30_terminal_error_paths_are_sanitized() {
    let mut w = World::new("30-error-sanitization", "tauri-and-rust");
    let unsafe_root = w.root.join("missing\n\u{1b}projects");
    w.set_config(
        "projects_dir",
        serde_json::json!(unsafe_root.to_string_lossy()),
    );

    let scan = w.run(&["scan"]);
    assert_eq!(scan.status.code(), Some(1));
    assert!(!scan.stdout.contains(&0x1b));
    assert!(!scan.stderr.contains(&0x1b));
    assert!(stderr(&scan).contains('\u{fffd}'));

    let clean = w.run(&["clean", "-y"]);
    assert_eq!(clean.status.code(), Some(1));
    assert!(!clean.stderr.contains(&0x1b));
    assert!(stderr(&clean).contains('\u{fffd}'));
    w.note("PASS: filesystem error paths are terminal-safe");
}

#[cfg(unix)]
#[test]
fn e2e_31_cargo_marker_symlink_does_not_authorize_clean() {
    let mut w = World::new("31-marker-symlink", "tauri-and-rust");
    let app = w.projects.join("unmarked");
    crate_at(&app, "unmarked", "");
    fs::create_dir_all(app.join("target/debug")).unwrap();
    file(&app.join("target/debug/keep"), 100);
    let outside_marker = w.root.join("valid-marker");
    fs::write(&outside_marker, CACHEDIR_TAG).unwrap();
    std::os::unix::fs::symlink(&outside_marker, app.join("target/CACHEDIR.TAG")).unwrap();

    let scan = w.run(&["scan", "--json"]);
    assert_ok(&scan, "scan with linked Cargo marker");
    assert!(json(&scan)["projects"].as_array().unwrap().is_empty());
    let clean = w.run(&["clean", "-y"]);
    assert_ok(&clean, "clean with linked Cargo marker");
    assert!(app.join("target/debug/keep").exists());
    assert_eq!(fs::read(&outside_marker).unwrap(), CACHEDIR_TAG.as_bytes());
    w.note("PASS: symlinked marker cannot authorize a target for cleanup");
}

#[cfg(not(target_os = "linux"))]
#[test]
fn e2e_32_trash_backend_fails_closed_without_mutating_the_target() {
    let mut w = World::new("32-trash-fail-closed", "tauri-and-rust");
    let app = w.projects.join("trash-app");
    crate_at(&app, "trash-app", "");
    mark_cargo_target(&app.join("target"));
    file(&app.join("target/debug/keep"), 100);
    w.set_config("clean_behavior", serde_json::json!("trash"));

    let dry = w.run(&["clean", "--dry-run", "--json"]);
    assert_eq!(
        dry.status.code(),
        Some(1),
        "unsupported Trash backend must fail closed in dry-run"
    );
    assert_eq!(json(&dry)["bytes"], 0);
    assert_eq!(json(&dry)["projects"][0]["result"], "error");
    assert!(app.join("target/debug/keep").exists());

    let clean = w.run(&["clean", "-y", "--json"]);
    assert_eq!(
        clean.status.code(),
        Some(1),
        "unsupported Trash identity backend must fail closed"
    );
    let result = &json(&clean)["projects"][0];
    assert_eq!(result["result"], "error");
    assert!(result["message"].as_str().unwrap().contains("Trash cannot"));
    assert_eq!(json(&clean)["bytes"], 0);
    assert!(app.join("target/debug/keep").exists());
    w.note("PASS: Trash backend refuses path-only deletion and leaves the target untouched");
}

#[test]
fn e2e_33_unresolved_workspace_member_preserves_ignored_shared_target() {
    let mut w = World::new("33-unresolved-workspace-member", "tauri-and-rust");
    let workspace = w.projects.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(
        workspace.join("Cargo.toml"),
        "[workspace]\nmembers = [\".hidden-member\", \"visible\"]\n",
    )
    .unwrap();
    let hidden = workspace.join(".hidden-member");
    fs::create_dir_all(hidden.join("src")).unwrap();
    fs::write(hidden.join("Cargo.toml"), "[package\ninvalid = true\n").unwrap();
    crate_at(&workspace.join("visible"), "visible", "");
    mark_cargo_target(&workspace.join("target"));
    file(&workspace.join("target/debug/shared"), 100);
    w.set_config(
        "ignored_projects",
        serde_json::json!(["possibly-protected-member"]),
    );

    let clean = w.run(&["clean", "-y"]);
    assert_ok(&clean, "clean with unresolved declared workspace member");
    assert!(workspace.join("target/debug/shared").exists());
    w.note("PASS: unresolved member cannot make ignored-project protection disappear");
}

#[cfg(target_os = "linux")]
#[test]
fn e2e_34_linux_trash_preserves_original_restore_path() {
    let mut w = World::new("34-linux-trash-restore-path", "tauri-and-rust");
    let app = w.projects.join("trash-app");
    crate_at(&app, "trash-app", "");
    mark_cargo_target(&app.join("target"));
    file(&app.join("target/debug/keep"), 100);
    let outside = w.root.join("outside-sentinel");
    fs::write(&outside, b"must survive").unwrap();
    std::os::unix::fs::symlink(&outside, app.join("target/debug/link")).unwrap();
    w.set_config("clean_behavior", serde_json::json!("trash"));

    let dry = w.run(&["clean", "--dry-run", "--json"]);
    assert_ok(&dry, "Linux Trash dry-run");
    assert_eq!(json(&dry)["projects"][0]["result"], "would-clean");
    assert!(app.join("target").exists());
    assert!(!w.home.join(".local/share/Trash").exists());

    let clean = w.run(&["clean", "-y", "--json"]);
    assert_ok(&clean, "Linux Trash clean");
    assert_eq!(json(&clean)["projects"][0]["result"], "cleaned");
    assert!(!app.join("target").exists());
    assert_eq!(fs::read(&outside).unwrap(), b"must survive");

    let trash_info = w.home.join(".local/share/Trash/info");
    let infos = fs::read_dir(&trash_info)
        .unwrap()
        .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<Vec<_>>();
    let original_path = app.join("target").to_string_lossy().into_owned();
    assert!(
        infos
            .iter()
            .any(|info| info.contains(&format!("Path={original_path}\n"))),
        "Trash metadata must retain the original artifact path: {infos:?}"
    );
    w.note("PASS: staged Trash entry preserves original path metadata and external link target");
}

#[cfg(target_os = "linux")]
#[test]
fn e2e_35_linux_trash_failure_restores_original_target() {
    let mut w = World::new("35-linux-trash-rollback", "tauri-and-rust");
    let app = w.projects.join("trash-app");
    crate_at(&app, "trash-app", "");
    mark_cargo_target(&app.join("target"));
    file(&app.join("target/debug/keep"), 100);
    let trash_root = w.home.join(".local/share/Trash");
    fs::create_dir_all(&trash_root).unwrap();
    fs::write(
        trash_root.join("info"),
        b"fixture blocks Trash metadata directory",
    )
    .unwrap();
    w.set_config("clean_behavior", serde_json::json!("trash"));

    let clean = w.run(&["clean", "-y", "--json"]);
    assert_eq!(
        clean.status.code(),
        Some(1),
        "Trash failure must be reported"
    );
    assert_eq!(json(&clean)["projects"][0]["result"], "error");
    assert!(app.join("target/debug/keep").exists());
    assert!(!fs::read_dir(&app).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".deoxidizer-trash-")));
    w.note("PASS: failed Trash move restores original target and removes empty staging directory");
}

#[cfg(windows)]
#[test]
fn e2e_36_windows_directory_link_leaf_is_removed_without_following() {
    let mut w = World::new("36-windows-directory-link", "tauri-and-rust");
    let app = w.projects.join("linked");
    crate_at(&app, "linked", "");
    mark_cargo_target(&app.join("target"));
    file(&app.join("target/debug/x"), 100);
    let outside = w.root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("sentinel"), b"must survive").unwrap();
    let link = app.join("target/directory-link");
    let link_text = link.to_string_lossy();
    let outside_text = outside.to_string_lossy();
    let status = std::process::Command::new("cmd")
        .args([
            "/C",
            "mklink",
            "/J",
            link_text.as_ref(),
            outside_text.as_ref(),
        ])
        .status();
    if status.map(|status| !status.success()).unwrap_or(true) {
        w.note("SKIP: this Windows account cannot create a junction");
        return;
    }

    let clean = w.run(&["clean", "-y"]);
    assert_ok(&clean, "clean with direct directory-link child");
    assert!(!app.join("target").exists());
    assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"must survive");
    w.note("PASS: directory-link child removed and external sentinel preserved");
}
