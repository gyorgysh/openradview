// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in local agent access. Constructing data/consent types opens no sockets.
pub mod broker;
pub mod catalog;
pub mod comparison;
pub mod connector;
pub mod consent;
pub mod discovery;
pub mod guidance;
pub mod http;
pub mod local_client;
pub mod stdio;
pub mod tools;
