//! Lager: a local web interface for editing an Ansible inventory database.
//!
//! [`inventory`] owns every rule about the data; [`web`] only translates HTTP
//! into calls on it and its results into pages.

pub mod inventory;
pub mod web;
