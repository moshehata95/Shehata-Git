// Copyright (c) 2026 Dr Mohamed Shehata. All rights reserved.
// Licensed under the MIT License. See LICENSE in the project root.

//! shehata-core — the shared brain of Shehata Git.
//!
//! Desktop (Tauri), CLI, credential helper, and MCP server all call into this
//! crate. Business logic never lives in command handlers or UI components.

pub mod accounts;
pub mod actions;
pub mod agents;
pub mod assignment;
pub mod audit;
pub mod diagnostics;
pub mod doctor;
pub mod error;
pub mod hooks;
pub mod integrations;
pub mod locking;
pub mod models;
pub mod prerequisites;
pub mod redact;
pub mod repositories;
pub mod routing;

pub use doctor::{Doctor, APP_VERSION};
pub use error::{Result, ShehataError};
pub use models::{AccountInfo, CheckStatus, DoctorReport, PushPolicy, SystemCheck};
