//! Marksman: a language server for Markdown.
//!
//! The module layout mirrors the original F# implementation one-to-one so that
//! the two can be read side by side.

pub mod ast;
pub mod code_actions;
pub mod compl;
pub mod config;
pub mod conn;
pub mod cst;
pub mod diag;
pub mod doc;
pub mod fatality;
pub mod folder;
pub mod gitignore;
pub mod graph;
pub mod index;
pub mod lenses;
pub mod mapping;
pub mod markdown;
pub mod misc;
pub mod mmap;
pub mod names;
pub mod parser;
pub mod partitioned_map;
pub mod paths;
pub mod refactor;
pub mod refs;
pub mod semato;
pub mod server;
pub mod state;
pub mod structure;
pub mod suffix_tree;
pub mod symbols;
pub mod syms;
pub mod text;
pub mod toc;
pub mod uri;
pub mod workspace;
