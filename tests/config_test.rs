use deoxidizer_lib::config::{CleanBehavior, Config, DefaultMode, Scope};

#[test]
fn test_default_config() {
    let config = Config::default();
    assert_eq!(config.version, 1);
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
        version: 1,
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
