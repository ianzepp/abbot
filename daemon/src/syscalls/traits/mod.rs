//! Traits Syscalls — inspect and modify personality traits at runtime.
//!
//! Provides `traits:list`, `traits:describe`, `traits:set`, and `traits:unset`
//! syscalls that allow agents to query and modify their trait configuration.
//! Changes persist to `abbot.toml` and take effect on the next session.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::app_config::{AppConfig, TraitsToml, default_config_path};
use crate::runtime::trait_catalog::trait_categories;

// ---------------------------------------------------------------------------
// Shared argument types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DescribeArgs {
    category: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SetArgs {
    category: String,
    variant: String,
}

#[derive(Debug, Deserialize)]
struct UnsetArgs {
    category: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Validate that a category name exists in the trait catalog.
fn validate_category(category: &str) -> Result<(), KernelError> {
    if trait_categories().iter().any(|(name, _)| *name == category) {
        Ok(())
    } else {
        Err(KernelError::invalid_args(format!(
            "unknown trait category: {category}"
        )))
    }
}

/// Validate that a variant exists within a category.
fn validate_variant(category: &str, variant: &str) -> Result<(), KernelError> {
    let cat = trait_categories()
        .iter()
        .find(|(name, _)| *name == category)
        .ok_or_else(|| KernelError::invalid_args(format!("unknown trait category: {category}")))?;

    if cat.1.contains(&variant) {
        Ok(())
    } else {
        Err(KernelError::invalid_args(format!(
            "unknown variant '{variant}' for category '{category}'"
        )))
    }
}

/// Set a trait field on a `TraitsToml` by category name.
fn set_trait_field(traits: &mut TraitsToml, category: &str, value: Option<String>) {
    match category {
        "fever" => traits.fever = value,
        "generation" => traits.generation = value,
        "autist" => traits.autist = value,
        "filter" => traits.filter = value,
        "poverty" => traits.poverty = value,
        "ego" => traits.ego = value,
        "paranoia" => traits.paranoia = value,
        "cultist" => traits.cultist = value,
        "dominance" => traits.dominance = value,
        "bipolar" => traits.bipolar = value,
        "xenophobe" => traits.xenophobe = value,
        "esoteric" => traits.esoteric = value,
        "collab" => traits.collab = value,
        _ => {}
    }
}

/// Load config from disk, apply a mutation, and save back.
fn mutate_config(f: impl FnOnce(&mut AppConfig)) -> Result<(), KernelError> {
    let path = default_config_path()
        .ok_or_else(|| KernelError::internal("cannot determine config path"))?;

    let mut config = AppConfig::load(&path);
    f(&mut config);
    config
        .save(&path)
        .map_err(|e| KernelError::io(e.to_string()))
}

// ---------------------------------------------------------------------------
// traits:list
// ---------------------------------------------------------------------------

pub struct TraitsList;

impl Default for TraitsList {
    fn default() -> Self {
        Self
    }
}

impl TraitsList {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TraitsList {
    fn name(&self) -> &'static str {
        "traits:list"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        _data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let traits = &AppConfig::global().traits;
        let mut active = serde_json::Map::new();

        for (cat, val) in [
            ("fever", &traits.fever),
            ("generation", &traits.generation),
            ("autist", &traits.autist),
            ("filter", &traits.filter),
            ("poverty", &traits.poverty),
            ("ego", &traits.ego),
            ("paranoia", &traits.paranoia),
            ("cultist", &traits.cultist),
            ("dominance", &traits.dominance),
            ("bipolar", &traits.bipolar),
            ("xenophobe", &traits.xenophobe),
            ("esoteric", &traits.esoteric),
            ("collab", &traits.collab),
        ] {
            if let Some(v) = val.as_deref().filter(|v| !v.is_empty() && *v != "none") {
                active.insert(cat.to_string(), json!(v));
            }
        }

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({ "traits": active })))
            .await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// traits:describe
// ---------------------------------------------------------------------------

pub struct TraitsDescribe;

impl Default for TraitsDescribe {
    fn default() -> Self {
        Self
    }
}

impl TraitsDescribe {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TraitsDescribe {
    fn name(&self) -> &'static str {
        "traits:describe"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let args: DescribeArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let categories: Vec<Value> = trait_categories()
            .iter()
            .filter(|(name, _)| {
                args.category
                    .as_deref()
                    .is_none_or(|filter| *name == filter)
            })
            .map(|(name, variants)| {
                json!({
                    "name": name,
                    "variants": variants,
                })
            })
            .collect();

        if categories.is_empty()
            && let Some(cat) = &args.category
        {
            return Err(KernelError::invalid_args(format!(
                "unknown trait category: {cat}"
            )));
        }

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({ "categories": categories })))
            .await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// traits:set
// ---------------------------------------------------------------------------

pub struct TraitsSet;

impl Default for TraitsSet {
    fn default() -> Self {
        Self
    }
}

impl TraitsSet {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TraitsSet {
    fn name(&self) -> &'static str {
        "traits:set"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        ctx.require_mutation()?;
        let args: SetArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        validate_variant(&args.category, &args.variant)?;

        let category = args.category.clone();
        let variant = args.variant.clone();

        mutate_config(|config| {
            set_trait_field(&mut config.traits, &category, Some(variant.clone()));
        })?;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({ "category": args.category, "variant": args.variant }),
            ))
            .await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// traits:unset
// ---------------------------------------------------------------------------

pub struct TraitsUnset;

impl Default for TraitsUnset {
    fn default() -> Self {
        Self
    }
}

impl TraitsUnset {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TraitsUnset {
    fn name(&self) -> &'static str {
        "traits:unset"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        ctx.require_mutation()?;
        let args: UnsetArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        validate_category(&args.category)?;

        let category = args.category.clone();
        mutate_config(|config| {
            set_trait_field(&mut config.traits, &category, None);
        })?;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({ "category": args.category, "removed": true }),
            ))
            .await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    dispatcher.register(Arc::new(TraitsList::new()));
    dispatcher.register(Arc::new(TraitsDescribe::new()));
    dispatcher.register(Arc::new(TraitsSet::new()));
    dispatcher.register(Arc::new(TraitsUnset::new()));
}
