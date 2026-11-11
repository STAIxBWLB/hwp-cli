//! The hwp compatibility executable, retained through the 1.x series.
#![recursion_limit = "256"]

#[path = "../commands/mod.rs"]
mod commands;
#[path = "../edit_ops.rs"]
mod edit_ops;
#[path = "../format.rs"]
mod format;
#[path = "../runtime.rs"]
mod runtime;

pub(crate) use runtime::WORKER_STACK_BYTES;

fn main() -> anyhow::Result<()> {
    runtime::run()
}
