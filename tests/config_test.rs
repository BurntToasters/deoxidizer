use deoxidizer_lib::config::{CleanBehavior, Config, ConfigError, DefaultMode, Scope};

#[test]
fn test_default_config() {
    let config = Config::default();
    assert_eq!(config.version, 2);
    assert_eq!(config.scope, Scope::TauriOnly);
    assert_eq!(config.clean_behavior, CleanBehavior::Delete);
    assert_eq!(config.default_mode, DefaultMode::Full);
    assert_eq!(config.min_size_mb, 0);
    assert!(config.ignored_projects.is_empty());
}

#[test]
fn test_config_roundtrip() {
    let directory = tempfile::tempdir().unwrap();
    let tmp = directory.path().join("deox_test_config.json");

    let config = Config {
        version: 2,
        projects_dir: "/tmp/test-projects".to_string(),
        scope: Scope::TauriAndRust,
        clean_behavior: CleanBehavior::Trash,
        default_mode: DefaultMode::DebugOnly,
        min_size_mb: 100,
        ignored_projects: vec!["my-project".to_string()],
    };

    config.save_to(&tmp).expect("save failed");
    let loaded = Config::load_from(&tmp).expect("load failed");

    assert_eq!(loaded.projects_dir, "/tmp/test-projects");
    assert_eq!(loaded.scope, Scope::TauriAndRust);
    assert_eq!(loaded.clean_behavior, CleanBehavior::Trash);
    assert_eq!(loaded.default_mode, DefaultMode::DebugOnly);
    assert_eq!(loaded.min_size_mb, 100);
    assert_eq!(loaded.ignored_projects, vec!["my-project"]);

    let mut replacement = loaded.clone();
    replacement.min_size_mb = 200;
    replacement.save_to(&tmp).expect("atomic overwrite failed");
    assert_eq!(Config::load_from(&tmp).unwrap().min_size_mb, 200);
}

#[test]
fn test_version_one_config_migrates() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy.json");
    std::fs::write(
        &path,
        r#"{
  "version": 1,
  "projects_dir": "/tmp/legacy-projects",
  "scope": "tauri-and-rust",
  "clean_behavior": "trash",
  "default_mode": "debug-only",
  "min_size_mb": 25,
  "ignored_projects": ["legacy-project"]
}"#,
    )
    .unwrap();

    let config = Config::load_from(&path).expect("legacy config should migrate");
    assert_eq!(config.version, 2);
    assert_eq!(config.projects_dir, "/tmp/legacy-projects");
    assert_eq!(config.scope, Scope::TauriAndRust);
    assert_eq!(config.clean_behavior, CleanBehavior::Trash);
    assert_eq!(config.default_mode, DefaultMode::DebugOnly);
    assert_eq!(config.min_size_mb, 25);
    assert_eq!(config.ignored_projects, vec!["legacy-project"]);
}

#[test]
fn test_missing_and_malformed_config_are_distinct() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.json");
    assert!(matches!(
        Config::load_from(&missing),
        Err(deoxidizer_lib::config::ConfigError::Missing(_))
    ));

    let malformed = directory.path().join("malformed.json");
    std::fs::write(&malformed, "{not-json").unwrap();
    assert!(matches!(
        Config::load_from(&malformed),
        Err(deoxidizer_lib::config::ConfigError::Parse(_))
    ));
}

#[test]
fn test_unsupported_config_version_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("future.json");
    std::fs::write(
        &path,
        r#"{
  "version": 99,
  "projects_dir": "/tmp",
  "scope": "tauri-only",
  "clean_behavior": "delete",
  "default_mode": "full",
  "min_size_mb": 0,
  "ignored_projects": []
}"#,
    )
    .unwrap();

    assert!(matches!(
        Config::load_from(&path),
        Err(deoxidizer_lib::config::ConfigError::UnsupportedVersion(99))
    ));
}

#[test]
fn test_scope_from_str() {
    assert_eq!(Scope::from_str_loose("tauri-only"), Some(Scope::TauriOnly));
    assert_eq!(Scope::from_str_loose("tauri"), Some(Scope::TauriOnly));
    assert_eq!(
        Scope::from_str_loose("tauri-and-rust"),
        Some(Scope::TauriAndRust)
    );
    assert_eq!(Scope::from_str_loose("both"), Some(Scope::TauriAndRust));
    assert_eq!(Scope::from_str_loose("rust-only"), Some(Scope::RustOnly));
    assert_eq!(Scope::from_str_loose("rust"), Some(Scope::RustOnly));
    assert_eq!(Scope::from_str_loose("invalid"), None);
}

#[test]
fn test_clean_behavior_from_str() {
    assert_eq!(
        CleanBehavior::from_str_loose("delete"),
        Some(CleanBehavior::Delete)
    );
    assert_eq!(
        CleanBehavior::from_str_loose("trash"),
        Some(CleanBehavior::Trash)
    );
    assert_eq!(
        CleanBehavior::from_str_loose("recycle"),
        Some(CleanBehavior::Trash)
    );
    assert_eq!(CleanBehavior::from_str_loose("invalid"), None);
}

#[test]
fn test_config_path() {
    let path = Config::config_path();
    assert!(path.to_string_lossy().contains(".deox_config"));
}

#[test]
fn test_tilde_expansion() {
    let mut config = Config {
        projects_dir: "~".to_string(),
        ..Default::default()
    };
    assert_eq!(config.projects_path(), dirs::home_dir().unwrap());

    config.projects_dir = "~/Projects".to_string();
    assert_eq!(
        config.projects_path(),
        dirs::home_dir().unwrap().join("Projects")
    );

    config.projects_dir = "/var/repos".to_string();
    assert_eq!(
        config.projects_path(),
        std::path::PathBuf::from("/var/repos")
    );
}

#[test]
fn test_default_mode_from_str_loose_aliases() {
    let cases: &[(&str, Option<DefaultMode>)] = &[
        ("full", Some(DefaultMode::Full)),
        ("FULL", Some(DefaultMode::Full)),
        ("debug-only", Some(DefaultMode::DebugOnly)),
        ("debug_only", Some(DefaultMode::DebugOnly)),
        ("debug", Some(DefaultMode::DebugOnly)),
        ("DEBUG", Some(DefaultMode::DebugOnly)),
        ("incremental-only", Some(DefaultMode::IncrementalOnly)),
        ("incremental_only", Some(DefaultMode::IncrementalOnly)),
        ("incremental", Some(DefaultMode::IncrementalOnly)),
        ("deps-only", Some(DefaultMode::DepsOnly)),
        ("deps_only", Some(DefaultMode::DepsOnly)),
        ("deps", Some(DefaultMode::DepsOnly)),
        ("invalid", None),
    ];
    for (input, expected) in cases {
        assert_eq!(
            DefaultMode::from_str_loose(input),
            *expected,
            "input: {input}"
        );
    }
}

#[test]
fn test_scope_and_behavior_loose_aliases() {
    assert_eq!(Scope::from_str_loose("all"), Some(Scope::TauriAndRust));
    assert_eq!(Scope::from_str_loose("ALL"), Some(Scope::TauriAndRust));
    assert_eq!(Scope::from_str_loose("BOTH"), Some(Scope::TauriAndRust));
    assert_eq!(
        CleanBehavior::from_str_loose("rm"),
        Some(CleanBehavior::Delete)
    );
    assert_eq!(
        CleanBehavior::from_str_loose("remove"),
        Some(CleanBehavior::Delete)
    );
    assert_eq!(
        CleanBehavior::from_str_loose("bin"),
        Some(CleanBehavior::Trash)
    );
    assert_eq!(
        CleanBehavior::from_str_loose("BIN"),
        Some(CleanBehavior::Trash)
    );
    assert_eq!(
        DefaultMode::from_str_loose("debug"),
        Some(DefaultMode::DebugOnly)
    );
    assert_eq!(
        DefaultMode::from_str_loose("incremental"),
        Some(DefaultMode::IncrementalOnly)
    );
    assert_eq!(
        DefaultMode::from_str_loose("deps"),
        Some(DefaultMode::DepsOnly)
    );
}

#[test]
fn test_validate_rejects_empty_projects_dir() {
    for dir in ["", "   "] {
        let config = Config {
            projects_dir: dir.to_string(),
            ..Default::default()
        };
        assert!(
            matches!(config.validate(), Err(ConfigError::Invalid(_))),
            "dir: {dir:?}"
        );
    }
}

#[test]
fn test_validate_rejects_min_size_overflow() {
    let config = Config {
        min_size_mb: u64::MAX,
        ..Default::default()
    };
    assert!(matches!(config.validate(), Err(ConfigError::Invalid(_))));
}

#[test]
fn test_unknown_fields_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("unknown.json");
    std::fs::write(
        &path,
        r#"{
  "version": 2,
  "projects_dir": "/tmp",
  "scope": "tauri-only",
  "clean_behavior": "delete",
  "default_mode": "full",
  "min_size_mb": 0,
  "ignored_projects": [],
  "unknown_field": 123
}"#,
    )
    .unwrap();
    assert!(matches!(
        Config::load_from(&path),
        Err(ConfigError::Parse(_))
    ));
}

#[test]
fn test_load_or_default_falls_back_when_missing() {
    let home = tempfile::tempdir().unwrap();
    let config_path = home.path().join(".deox_config");

    let result = Config::load_or_default_from(&config_path);

    let config = result.expect("missing config should fall back to defaults");
    assert_eq!(config.version, 2);
}

#[test]
fn test_load_or_default_propagates_malformed() {
    let home = tempfile::tempdir().unwrap();
    let config_path = home.path().join(".deox_config");
    std::fs::write(&config_path, "{not-json").unwrap();

    let result = Config::load_or_default_from(&config_path);

    assert!(matches!(result, Err(ConfigError::Parse(_))));
}

#[test]
fn test_v1_without_ignored_projects_migrates() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy-no-ignore.json");
    std::fs::write(
        &path,
        r#"{
  "version": 1,
  "projects_dir": "/tmp/legacy-projects",
  "scope": "tauri-only",
  "clean_behavior": "delete",
  "default_mode": "full",
  "min_size_mb": 0
}"#,
    )
    .unwrap();

    // ignored_projects defaults when absent from v1.
    let config = Config::load_from(&path).expect("v1 without ignored_projects must migrate");
    assert_eq!(config.version, 2);
    assert!(config.ignored_projects.is_empty());
}

#[test]
fn test_v1_unknown_field_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy-unknown.json");
    std::fs::write(
        &path,
        r#"{
  "version": 1,
  "projects_dir": "/tmp/legacy-projects",
  "scope": "tauri-only",
  "clean_behavior": "delete",
  "default_mode": "full",
  "min_size_mb": 0,
  "unknown_field": 123
}"#,
    )
    .unwrap();

    assert!(matches!(
        Config::load_from(&path),
        Err(ConfigError::Parse(_))
    ));
}

#[test]
fn test_missing_version_and_zero_version_invalid() {
    let directory = tempfile::tempdir().unwrap();

    let missing = directory.path().join("no-version.json");
    std::fs::write(
        &missing,
        r#"{
  "projects_dir": "/tmp",
  "scope": "tauri-only",
  "clean_behavior": "delete",
  "default_mode": "full",
  "min_size_mb": 0,
  "ignored_projects": []
}"#,
    )
    .unwrap();
    assert!(matches!(
        Config::load_from(&missing),
        Err(ConfigError::Invalid(_))
    ));

    let zero = directory.path().join("zero-version.json");
    std::fs::write(
        &zero,
        r#"{
  "version": 0,
  "projects_dir": "/tmp",
  "scope": "tauri-only",
  "clean_behavior": "delete",
  "default_mode": "full",
  "min_size_mb": 0,
  "ignored_projects": []
}"#,
    )
    .unwrap();
    assert!(matches!(
        Config::load_from(&zero),
        Err(ConfigError::UnsupportedVersion(0))
    ));

    let stringy = directory.path().join("string-version.json");
    std::fs::write(
        &stringy,
        r#"{
  "version": "2",
  "projects_dir": "/tmp",
  "scope": "tauri-only",
  "clean_behavior": "delete",
  "default_mode": "full",
  "min_size_mb": 0,
  "ignored_projects": []
}"#,
    )
    .unwrap();
    assert!(matches!(
        Config::load_from(&stringy),
        Err(ConfigError::Invalid(_))
    ));
}

#[test]
fn test_save_to_rejects_invalid_without_truncating() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    let valid = Config {
        projects_dir: "/tmp/projects".to_string(),
        ..Default::default()
    };
    valid.save_to(&path).expect("valid save must succeed");
    let before = std::fs::read(&path).unwrap();

    let empty_dir = Config {
        projects_dir: String::new(),
        ..Default::default()
    };
    assert!(empty_dir.save_to(&path).is_err());

    let overflow = Config {
        min_size_mb: u64::MAX,
        ..Default::default()
    };
    assert!(overflow.save_to(&path).is_err());

    // Validation happens before any write: original bytes must be intact.
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn test_tilde_user_and_relative_passthrough() {
    let mut config = Config {
        projects_dir: "~other/x".to_string(),
        ..Default::default()
    };
    assert_eq!(config.projects_path(), std::path::PathBuf::from("~other/x"));

    config.projects_dir = "relative/path".to_string();
    assert_eq!(
        config.projects_path(),
        std::path::PathBuf::from("relative/path")
    );
}
