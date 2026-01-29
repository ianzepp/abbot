use std::path::PathBuf;
use clap::Parser;

/// Configuration for the monastery server
#[derive(Parser, Clone)]
#[command(name = "abbot-server")]
#[command(about = "AI Monastery Server - background process for monk orchestration")]
pub struct Config {
    /// Path to monastery directory (contains monks/, hermitage/, database)
    #[arg(short, long, env = "ABBOT_MONASTERY", default_value = ".")]
    pub monastery: PathBuf,

    /// Path to order directory (contains grammar.md, system.md, shared rules)
    #[arg(short, long, env = "ABBOT_ORDER")]
    pub order: Option<PathBuf>,

    /// Path to database file (relative to monastery, or absolute)
    #[arg(short, long, env = "ABBOT_DB", default_value = "abbot.db")]
    pub db: PathBuf,

    /// IRC server port
    #[arg(short, long, env = "ABBOT_PORT", default_value = "6667")]
    pub port: u16,
}

impl Config {
    /// Get the monastery path, resolving to absolute
    pub fn monastery_path(&self) -> PathBuf {
        self.monastery.canonicalize().unwrap_or_else(|_| self.monastery.clone())
    }

    /// Get the order path - if not specified, defaults to monastery/order/
    pub fn order_path(&self) -> PathBuf {
        match &self.order {
            Some(p) => p.clone(),
            None => self.monastery.join("order"),
        }
    }

    /// Path to the monks directory
    pub fn monks_dir(&self) -> PathBuf {
        self.monastery.join("monks")
    }

    /// Path to the hermitage root
    pub fn hermitage_root(&self) -> PathBuf {
        self.monastery.join("hermitage")
    }

    /// Path to the database
    pub fn db_path(&self) -> PathBuf {
        if self.db.is_absolute() {
            self.db.clone()
        } else {
            self.monastery.join(&self.db)
        }
    }

    /// Path to grammar.md (in order/)
    pub fn grammar_path(&self) -> PathBuf {
        self.order_path().join("grammar.md")
    }

    /// Path to system.md (in order/)
    pub fn system_path(&self) -> PathBuf {
        self.order_path().join("system.md")
    }

    /// Path to rector.md (in order/)
    pub fn rector_path(&self) -> PathBuf {
        self.order_path().join("rector.md")
    }

    /// Load grammar.md content
    pub fn load_grammar(&self) -> String {
        std::fs::read_to_string(self.grammar_path())
            .unwrap_or_else(|_| {
                tracing::warn!("grammar.md not found at {:?}, using default", self.grammar_path());
                include_str!("../order/grammar.md").to_string()
            })
    }

    /// Load system.md content
    pub fn load_system(&self) -> String {
        std::fs::read_to_string(self.system_path())
            .unwrap_or_else(|_| {
                tracing::warn!("system.md not found at {:?}, using default", self.system_path());
                include_str!("../order/system.md").to_string()
            })
    }
}

/// Global configuration access for modules that need paths
///
/// This is set once at startup and then accessed via config::get()
use std::sync::OnceLock;

static GLOBAL_CONFIG: OnceLock<Config> = OnceLock::new();

pub fn init(config: Config) {
    let _ = GLOBAL_CONFIG.set(config);
}

pub fn get() -> &'static Config {
    GLOBAL_CONFIG.get().expect("config not initialized")
}

/// Helper to get monks directory from global config
pub fn monks_dir() -> PathBuf {
    get().monks_dir()
}

/// Helper to get hermitage root from global config
pub fn hermitage_root() -> PathBuf {
    get().hermitage_root()
}
