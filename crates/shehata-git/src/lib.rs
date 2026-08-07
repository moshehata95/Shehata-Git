// Copyright (c) 2026 Dr Mohamed Shehata. All rights reserved.
// Licensed under the MIT License. See LICENSE in the project root.

//! shehata-git — safe wrapper around the system `git` executable.
//!
//! Rules enforced here:
//! - Commands always run with argument arrays, never shell strings.
//! - Output is treated as data; nothing is re-executed.
//! - Destructive operations are not exposed by this crate at all.

pub mod remote;
pub mod repository;
pub mod runner;

pub use remote::{parse_remote_url, RemoteProtocol, RemoteUrl};
pub use repository::{
    discover_repository, read_local_config_values, replace_local_config_values,
    DiscoveredRepository, RepositoryDiscoveryError, RepositoryRemote, RepositoryRemoteProtocol,
    WorktreeStatus,
};
pub use runner::{CommandOutput, GitError, GitRunner, INTERNAL_MARKER};
