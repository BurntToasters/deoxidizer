//! `deox` — short alias for `deoxidizer`.
//!
//! This binary has identical behavior to `deoxidizer`. Both commands share
//! the same library, CLI parser, configuration, and update logic.

fn main() {
    deoxidizer_lib::cli::run_app();
}
