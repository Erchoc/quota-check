//! CLI implementation, kept in the library so both installed binaries
//! (`quota-check` and the short `qc` alias) are one-line wrappers around it.

mod cli;

pub use cli::run;
