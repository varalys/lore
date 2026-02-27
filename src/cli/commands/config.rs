//! Config command - view and manage Lore configuration.
//!
//! Provides subcommands to show, get, and set configuration values.
//! Configuration is stored in ~/.lore/config.yaml.

use anyhow::{bail, Result};
use colored::Colorize;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::config::Config;
use crate::storage::db::default_db_path;
use crate::storage::{Database, Machine};

/// Arguments for the config command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore config                          Show configuration paths and settings\n    \
    lore config get watchers             Get the value of a config key\n    \
    lore config set watchers claude-code,aider  Set enabled watchers\n    \
    lore config --format json            Output as JSON")]
pub struct Args {
    /// Config subcommand
    #[command(subcommand)]
    pub command: Option<ConfigCommand>,

    /// Output format: text (default), json
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

/// Config subcommands.
#[derive(clap::Subcommand)]
pub enum ConfigCommand {
    /// Get a configuration value
    Get {
        /// The configuration key to get
        key: String,
    },
    /// Set a configuration value
    Set {
        /// The configuration key to set
        key: String,
        /// The value to set
        value: String,
    },
}

/// JSON output structure for config show.
#[derive(Serialize)]
struct ConfigShowOutput {
    database_path: String,
    config_path: String,
    config_exists: bool,
    settings: ConfigSettings,
}

/// JSON representation of config settings.
#[derive(Serialize)]
struct ConfigSettings {
    machine_id: Option<String>,
    machine_name: Option<String>,
    watchers: Vec<String>,
    auto_link: bool,
    auto_link_threshold: f64,
    commit_footer: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary_model_anthropic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary_model_openai: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary_model_openrouter: Option<String>,
    summary_auto: bool,
    summary_auto_threshold: usize,
}

/// Executes the config command.
pub fn run(args: Args) -> Result<()> {
    match args.command {
        Some(ConfigCommand::Get { key }) => run_get(&key, args.format),
        Some(ConfigCommand::Set { key, value }) => run_set(&key, &value),
        None => run_show(args.format),
    }
}

/// Shows the current configuration.
fn run_show(format: OutputFormat) -> Result<()> {
    let db_path = default_db_path()?;
    let config_path = Config::config_path()?;
    let config_exists = config_path.exists();
    let config = Config::load()?;

    match format {
        OutputFormat::Json => {
            let output = ConfigShowOutput {
                database_path: db_path.display().to_string(),
                config_path: config_path.display().to_string(),
                config_exists,
                settings: ConfigSettings {
                    machine_id: config.machine_id.clone(),
                    machine_name: config.machine_name.clone(),
                    watchers: config.watchers.clone(),
                    auto_link: config.auto_link,
                    auto_link_threshold: config.auto_link_threshold,
                    commit_footer: config.commit_footer,
                    summary_provider: config.summary_provider.clone(),
                    summary_model_anthropic: config.summary_model_anthropic.clone(),
                    summary_model_openai: config.summary_model_openai.clone(),
                    summary_model_openrouter: config.summary_model_openrouter.clone(),
                    summary_auto: config.summary_auto,
                    summary_auto_threshold: config.summary_auto_threshold,
                },
            };
            let json = serde_json::to_string_pretty(&output)?;
            println!("{json}");
        }
        OutputFormat::Text | OutputFormat::Markdown => {
            println!("{}", "Lore Configuration".bold());
            println!();
            println!("{}", "Paths:".dimmed());
            println!("  Database:     {}", db_path.display());
            print!("  Config file:  {}", config_path.display());
            if !config_exists {
                print!(" {}", "(not created)".dimmed());
            }
            println!();
            println!();
            println!("{}", "Machine Identity:".dimmed());
            println!(
                "  machine_id:   {}",
                config
                    .machine_id
                    .as_deref()
                    .map(|s| s.cyan().to_string())
                    .unwrap_or_else(|| "(not set)".dimmed().to_string())
            );
            println!(
                "  machine_name: {}",
                config
                    .machine_name
                    .as_deref()
                    .map(|s| s.cyan().to_string())
                    .unwrap_or_else(|| "(not set)".dimmed().to_string())
            );
            println!();
            println!("{}", "Settings:".dimmed());
            println!(
                "  watchers:            {}",
                if config.watchers.is_empty() {
                    "(none)".dimmed().to_string()
                } else {
                    config.watchers.join(", ").cyan().to_string()
                }
            );
            println!(
                "  auto_link:           {}",
                if config.auto_link {
                    "true".green()
                } else {
                    "false".yellow()
                }
            );
            println!(
                "  auto_link_threshold: {}",
                format!("{:.1}", config.auto_link_threshold).cyan()
            );
            println!(
                "  commit_footer:       {}",
                if config.commit_footer {
                    "true".green()
                } else {
                    "false".yellow()
                }
            );
            println!();

            // Summary settings (only show section if any summary config exists)
            let has_summary_config = config.summary_provider.is_some()
                || config.summary_api_key_anthropic.is_some()
                || config.summary_api_key_openai.is_some()
                || config.summary_api_key_openrouter.is_some();

            if has_summary_config {
                println!("{}", "Summary:".dimmed());
                println!(
                    "  summary_provider:    {}",
                    config
                        .summary_provider
                        .as_deref()
                        .map(|s| s.cyan().to_string())
                        .unwrap_or_else(|| "(not set)".dimmed().to_string())
                );

                // Show which providers have keys configured (masked)
                let providers = [
                    ("anthropic", &config.summary_api_key_anthropic),
                    ("openai", &config.summary_api_key_openai),
                    ("openrouter", &config.summary_api_key_openrouter),
                ];
                for (name, key) in &providers {
                    if let Some(k) = key {
                        println!(
                            "  summary_api_key_{}: {}",
                            format!("{name:<10}"),
                            mask_secret(k).dimmed()
                        );
                    }
                }

                // Show custom models if set
                let models = [
                    ("anthropic", &config.summary_model_anthropic),
                    ("openai", &config.summary_model_openai),
                    ("openrouter", &config.summary_model_openrouter),
                ];
                for (name, model) in &models {
                    if let Some(m) = model {
                        println!(
                            "  summary_model_{}: {}",
                            format!("{name:<11}"),
                            m.cyan()
                        );
                    }
                }

                println!(
                    "  summary_auto:        {}",
                    if config.summary_auto {
                        "true".green()
                    } else {
                        "false".yellow()
                    }
                );
                if config.summary_auto {
                    println!(
                        "  summary_auto_threshold: {}",
                        config.summary_auto_threshold.to_string().cyan()
                    );
                }
                println!();
            }

            println!(
                "{}",
                "Use 'lore config set <key> <value>' to change settings.".dimmed()
            );
        }
    }

    Ok(())
}

/// Gets a configuration value by key.
fn run_get(key: &str, format: OutputFormat) -> Result<()> {
    let config = Config::load()?;

    let value = config.get(key);

    match value {
        Some(v) => {
            // Mask sensitive values in output
            let display_value = if key.starts_with("summary_api_key") {
                mask_secret(&v)
            } else {
                v.clone()
            };

            match format {
                OutputFormat::Json => {
                    let output = serde_json::json!({ "key": key, "value": display_value });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
                OutputFormat::Text | OutputFormat::Markdown => {
                    println!("{display_value}");
                }
            }
            Ok(())
        }
        None => {
            let valid_keys = Config::valid_keys();
            bail!(
                "Unknown configuration key: '{}'\n\nValid keys: {}",
                key,
                valid_keys.join(", ")
            );
        }
    }
}

/// Sets a configuration value by key.
fn run_set(key: &str, value: &str) -> Result<()> {
    let config_path = Config::config_path()?;
    let mut config = Config::load()?;

    config.set(key, value)?;
    config.save_to_path(&config_path)?;

    // If setting machine_name, also update the machines table
    if key == "machine_name" {
        if let Ok(machine_id) = config.get_or_create_machine_id() {
            if let Ok(db) = Database::open_default() {
                let machine = Machine {
                    id: machine_id,
                    name: value.to_string(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                };
                // Ignore errors here since config update was successful
                let _ = db.upsert_machine(&machine);
            }
        }
    }

    // Mask sensitive values in output
    let display_value = if key.starts_with("summary_api_key") {
        mask_secret(value)
    } else {
        value.to_string()
    };
    println!(
        "{} {} = {}",
        "Set".green(),
        key.cyan(),
        display_value.cyan()
    );

    Ok(())
}

/// Masks a secret value for display, showing only the first 4 and last 4 characters.
///
/// Short values (12 characters or fewer) are fully masked.
fn mask_secret(value: &str) -> String {
    if value.len() <= 12 {
        "*".repeat(value.len())
    } else {
        format!("{}...{}", &value[..4], &value[value.len() - 4..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_config(dir: &TempDir) -> (std::path::PathBuf, Config) {
        let config_path = dir.path().join("config.yaml");
        let config = Config::default();
        config.save_to_path(&config_path).unwrap();
        (config_path, config)
    }

    #[test]
    fn test_config_show_output_structure() {
        let output = ConfigShowOutput {
            database_path: "/test/db".to_string(),
            config_path: "/test/config".to_string(),
            config_exists: true,
            settings: ConfigSettings {
                machine_id: Some("test-uuid".to_string()),
                machine_name: Some("test-machine".to_string()),
                watchers: vec!["claude-code".to_string()],
                auto_link: false,
                auto_link_threshold: 0.7,
                commit_footer: false,
                summary_provider: None,
                summary_model_anthropic: None,
                summary_model_openai: None,
                summary_model_openrouter: None,
                summary_auto: false,
                summary_auto_threshold: 4,
            },
        };

        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("database_path"));
        assert!(json.contains("config_path"));
        assert!(json.contains("settings"));
        assert!(json.contains("watchers"));
    }

    #[test]
    fn test_config_settings_serialization() {
        let settings = ConfigSettings {
            machine_id: Some("test-uuid".to_string()),
            machine_name: Some("test-machine".to_string()),
            watchers: vec!["aider".to_string(), "claude-code".to_string()],
            auto_link: true,
            auto_link_threshold: 0.8,
            commit_footer: true,
            summary_provider: Some("anthropic".to_string()),
            summary_model_anthropic: None,
            summary_model_openai: None,
            summary_model_openrouter: None,
            summary_auto: false,
            summary_auto_threshold: 4,
        };

        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("aider"));
        assert!(json.contains("claude-code"));
        assert!(json.contains("true"));
        assert!(json.contains("0.8"));
        assert!(json.contains("summary_provider"));
        assert!(json.contains("anthropic"));
    }

    #[test]
    fn test_run_set_updates_config() {
        let temp_dir = TempDir::new().unwrap();
        let (config_path, _) = create_test_config(&temp_dir);

        // Load, set, and save manually to test the flow
        let mut config = Config::load_from_path(&config_path).unwrap();
        config.set("auto_link", "true").unwrap();
        config.save_to_path(&config_path).unwrap();

        // Reload and verify
        let reloaded = Config::load_from_path(&config_path).unwrap();
        assert!(reloaded.auto_link);
    }

    #[test]
    fn test_run_set_watchers() {
        let temp_dir = TempDir::new().unwrap();
        let (config_path, _) = create_test_config(&temp_dir);

        let mut config = Config::load_from_path(&config_path).unwrap();
        config.set("watchers", "aider,claude-code,cline").unwrap();
        config.save_to_path(&config_path).unwrap();

        let reloaded = Config::load_from_path(&config_path).unwrap();
        assert_eq!(
            reloaded.watchers,
            vec![
                "aider".to_string(),
                "claude-code".to_string(),
                "cline".to_string()
            ]
        );
    }

    #[test]
    fn test_mask_secret_long_value() {
        let masked = mask_secret("sk-ant-api03-abcdef123456");
        assert_eq!(masked, "sk-a...3456");
    }

    #[test]
    fn test_mask_secret_short_value() {
        let masked = mask_secret("short");
        assert_eq!(masked, "*****");
    }

    #[test]
    fn test_mask_secret_exactly_12_chars() {
        let masked = mask_secret("123456789012");
        assert_eq!(masked, "************");
    }

    #[test]
    fn test_mask_secret_13_chars() {
        let masked = mask_secret("1234567890123");
        assert_eq!(masked, "1234...0123");
    }
}
