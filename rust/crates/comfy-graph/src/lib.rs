//! Rust port of ComfyUI's execution-graph model.
//!
//! This crate mirrors the behavior of `comfy_execution/graph_utils.py` and the
//! prompt-graph portions of `comfy_execution/graph.py`. Graphs use the same
//! JSON shape as the ComfyUI API prompt format, so a serialized graph produced
//! here can be submitted to the existing Python backend unchanged.

pub mod graph;
pub mod graph_utils;

pub use graph::{DynamicPrompt, GraphError, NodeDefResolver, NoopResolver, TopologicalSort};
pub use graph_utils::{add_graph_prefix, is_link, ExecutionBlocker, GraphBuilder, Link, Node};
