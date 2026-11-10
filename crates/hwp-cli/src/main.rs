//! Canonical ahwp executable.
#![recursion_limit = "256"]

mod commands;
mod edit_ops;
mod format;
mod runtime;

pub(crate) use runtime::WORKER_STACK_BYTES;

fn main() -> anyhow::Result<()> {
    runtime::run()
}
