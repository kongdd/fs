//! Isolated experimental engine; the production CLI does not depend on this crate.
#[path = "../../src/config.rs"]
pub mod config;
#[path = "../../src/database.rs"]
mod database;
#[path = "../../src/everything.rs"]
mod everything;
pub mod indexer;
#[path = "../../src/matching.rs"]
mod matching;
pub mod native;
pub mod search;
pub mod stats;
#[path = "../../src/ui.rs"]
pub mod ui;
