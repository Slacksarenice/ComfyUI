//! Port of the prompt-graph portions of `comfy_execution/graph.py`.
//!
//! The Python `TopologicalSort` reaches into the global node registry
//! (`nodes.NODE_CLASS_MAPPINGS`) to ask whether an input is lazy. That lookup
//! is abstracted here behind [`NodeDefResolver`] so the graph logic stays free
//! of any node-registry or Python dependency.

use indexmap::{IndexMap, IndexSet};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::graph_utils::{is_link, Link};

#[derive(Debug, Error)]
pub enum GraphError {
    #[error("Dependency cycle detected")]
    DependencyCycle { nodes_in_cycle: Vec<String> },
    #[error("{0}")]
    NodeInput(String),
    #[error("Node {0} not found")]
    NodeNotFound(String),
}

/// Resolves node-definition metadata that lives in the node registry on the
/// Python side. Implementations can consult a native registry, ask the Python
/// backend over IPC, or (for pure graph work) use [`NoopResolver`].
pub trait NodeDefResolver {
    /// Whether the given input of the given node class is declared lazy.
    fn is_input_lazy(&self, _class_type: &str, _input_name: &str) -> bool {
        false
    }
    /// Whether the node class is an output node (`OUTPUT_NODE = True`).
    fn is_output_node(&self, _class_type: &str) -> bool {
        false
    }
}

/// Resolver that treats every input as non-lazy and no node as an output
/// node. Matches graphs that do not use lazy evaluation.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopResolver;

impl NodeDefResolver for NoopResolver {}

/// Port of the Python `DynamicPrompt`: the user's original prompt plus any
/// ephemeral nodes added while the graph executes (e.g. expanded subgraphs).
#[derive(Debug, Default)]
pub struct DynamicPrompt {
    original_prompt: Map<String, Value>,
    ephemeral_prompt: Map<String, Value>,
    ephemeral_parents: IndexMap<String, String>,
    ephemeral_display: IndexMap<String, String>,
}

impl DynamicPrompt {
    pub fn new(original_prompt: Map<String, Value>) -> Self {
        Self {
            original_prompt,
            ..Default::default()
        }
    }

    pub fn get_node(&self, node_id: &str) -> Result<&Value, GraphError> {
        self.ephemeral_prompt
            .get(node_id)
            .or_else(|| self.original_prompt.get(node_id))
            .ok_or_else(|| GraphError::NodeNotFound(node_id.to_string()))
    }

    pub fn has_node(&self, node_id: &str) -> bool {
        self.original_prompt.contains_key(node_id) || self.ephemeral_prompt.contains_key(node_id)
    }

    pub fn add_ephemeral_node(
        &mut self,
        node_id: &str,
        node_info: Value,
        parent_id: &str,
        display_id: &str,
    ) {
        self.ephemeral_prompt.insert(node_id.to_string(), node_info);
        self.ephemeral_parents
            .insert(node_id.to_string(), parent_id.to_string());
        self.ephemeral_display
            .insert(node_id.to_string(), display_id.to_string());
    }

    pub fn get_real_node_id<'a>(&'a self, mut node_id: &'a str) -> &'a str {
        while let Some(parent) = self.ephemeral_parents.get(node_id) {
            node_id = parent;
        }
        node_id
    }

    pub fn get_parent_node_id(&self, node_id: &str) -> Option<&str> {
        self.ephemeral_parents.get(node_id).map(|s| s.as_str())
    }

    pub fn get_display_node_id<'a>(&'a self, mut node_id: &'a str) -> &'a str {
        while let Some(display) = self.ephemeral_display.get(node_id) {
            node_id = display;
        }
        node_id
    }

    pub fn all_node_ids(&self) -> IndexSet<String> {
        self.original_prompt
            .keys()
            .chain(self.ephemeral_prompt.keys())
            .cloned()
            .collect()
    }

    pub fn get_original_prompt(&self) -> &Map<String, Value> {
        &self.original_prompt
    }

    fn node_class_type(&self, node_id: &str) -> Result<String, GraphError> {
        let node = self.get_node(node_id)?;
        Ok(node
            .get("class_type")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string())
    }

    fn node_inputs(&self, node_id: &str) -> Result<&Map<String, Value>, GraphError> {
        let node = self.get_node(node_id)?;
        node.get("inputs")
            .and_then(|v| v.as_object())
            .ok_or_else(|| GraphError::NodeInput(format!("Node {node_id} has no inputs object")))
    }
}

/// Port of the Python `TopologicalSort`: a dependency tracker that dissolves
/// the graph as nodes complete. `blocking[from][to]` records which sockets of
/// `from` the node `to` is waiting on; `block_count[to]` is the number of
/// distinct nodes blocking `to`.
pub struct TopologicalSort<'a, R: NodeDefResolver> {
    pub dynprompt: &'a DynamicPrompt,
    resolver: &'a R,
    pending_nodes: IndexMap<String, ()>,
    block_count: IndexMap<String, usize>,
    blocking: IndexMap<String, IndexMap<String, IndexMap<u32, bool>>>,
    /// Hook mirroring `ExecutionList.is_cached`: node ids present here are
    /// treated as already computed and never re-added to the graph.
    cached_nodes: IndexSet<String>,
}

impl<'a, R: NodeDefResolver> TopologicalSort<'a, R> {
    pub fn new(dynprompt: &'a DynamicPrompt, resolver: &'a R) -> Self {
        Self {
            dynprompt,
            resolver,
            pending_nodes: IndexMap::new(),
            block_count: IndexMap::new(),
            blocking: IndexMap::new(),
            cached_nodes: IndexSet::new(),
        }
    }

    /// Mark a node as cached so links from it never become dependencies.
    pub fn set_cached(&mut self, node_id: &str) {
        self.cached_nodes.insert(node_id.to_string());
    }

    pub fn is_cached(&self, node_id: &str) -> bool {
        self.cached_nodes.contains(node_id)
    }

    pub fn make_input_strong_link(
        &mut self,
        to_node_id: &str,
        to_input: &str,
    ) -> Result<(), GraphError> {
        let inputs = self.dynprompt.node_inputs(to_node_id)?;
        let value = inputs.get(to_input).ok_or_else(|| {
            GraphError::NodeInput(format!(
                "Node {to_node_id} says it needs input {to_input}, but there is no input to that node at all"
            ))
        })?;
        let link = Link::parse(value).ok_or_else(|| {
            GraphError::NodeInput(format!(
                "Node {to_node_id} says it needs input {to_input}, but that value is a constant"
            ))
        })?;
        self.add_strong_link(&link.node_id, link.socket, to_node_id)
    }

    pub fn add_strong_link(
        &mut self,
        from_node_id: &str,
        from_socket: u32,
        to_node_id: &str,
    ) -> Result<(), GraphError> {
        if self.is_cached(from_node_id) {
            return Ok(());
        }
        self.add_node(from_node_id, false, None)?;
        let blocking = self.blocking.get_mut(from_node_id).unwrap();
        let entry = blocking.entry(to_node_id.to_string());
        let newly_blocked = matches!(entry, indexmap::map::Entry::Vacant(_));
        let sockets = entry.or_default();
        if newly_blocked {
            *self.block_count.get_mut(to_node_id).unwrap() += 1;
        }
        sockets.insert(from_socket, true);
        Ok(())
    }

    /// Add a node and (transitively) all of its non-lazy link dependencies.
    pub fn add_node(
        &mut self,
        node_unique_id: &str,
        include_lazy: bool,
        subgraph_nodes: Option<&IndexSet<String>>,
    ) -> Result<(), GraphError> {
        let mut node_ids = vec![node_unique_id.to_string()];
        let mut links: Vec<(String, u32, String)> = Vec::new();

        while let Some(unique_id) = node_ids.pop() {
            if self.pending_nodes.contains_key(&unique_id) {
                continue;
            }
            self.pending_nodes.insert(unique_id.clone(), ());
            self.block_count.insert(unique_id.clone(), 0);
            self.blocking.insert(unique_id.clone(), IndexMap::new());

            let class_type = self.dynprompt.node_class_type(&unique_id)?;
            let inputs = self.dynprompt.node_inputs(&unique_id)?.clone();
            for (input_name, value) in &inputs {
                if !is_link(value) {
                    continue;
                }
                let link = Link::parse(value).ok_or_else(|| {
                    GraphError::NodeInput(format!(
                        "Node {unique_id} input {input_name} has a non-integer socket index"
                    ))
                })?;
                if let Some(subgraph) = subgraph_nodes {
                    if !subgraph.contains(&link.node_id) {
                        continue;
                    }
                }
                let is_lazy = self.resolver.is_input_lazy(&class_type, input_name);
                if include_lazy || !is_lazy {
                    if !self.is_cached(&link.node_id) {
                        node_ids.push(link.node_id.clone());
                    }
                    links.push((link.node_id, link.socket, unique_id.clone()));
                }
            }
        }

        for (from_node_id, from_socket, to_node_id) in links {
            self.add_strong_link(&from_node_id, from_socket, &to_node_id)?;
        }
        Ok(())
    }

    /// Nodes with no unresolved dependencies, in insertion order.
    pub fn get_ready_nodes(&self) -> Vec<String> {
        self.pending_nodes
            .keys()
            .filter(|node_id| self.block_count[*node_id] == 0)
            .cloned()
            .collect()
    }

    /// Remove a completed node and unblock everything it was blocking.
    pub fn pop_node(&mut self, unique_id: &str) {
        self.pending_nodes.shift_remove(unique_id);
        if let Some(blocked) = self.blocking.shift_remove(unique_id) {
            for blocked_node_id in blocked.keys() {
                if let Some(count) = self.block_count.get_mut(blocked_node_id) {
                    *count -= 1;
                }
            }
        }
        self.block_count.shift_remove(unique_id);
    }

    pub fn is_empty(&self) -> bool {
        self.pending_nodes.is_empty()
    }

    /// Pop every ready node in dependency order. Returns the full execution
    /// order, or a [`GraphError::DependencyCycle`] naming the cycle members.
    pub fn execution_order(&mut self) -> Result<Vec<String>, GraphError> {
        let mut order = Vec::new();
        while !self.is_empty() {
            let ready = self.get_ready_nodes();
            if ready.is_empty() {
                return Err(GraphError::DependencyCycle {
                    nodes_in_cycle: self.get_nodes_in_cycle(),
                });
            }
            for node_id in ready {
                order.push(node_id.clone());
                self.pop_node(&node_id);
            }
        }
        Ok(order)
    }

    /// Dissolve the graph in reverse topological order, leaving only the
    /// nodes participating in a cycle. Port of `get_nodes_in_cycle`.
    pub fn get_nodes_in_cycle(&self) -> Vec<String> {
        let mut blocked_by: IndexMap<String, IndexSet<String>> = self
            .pending_nodes
            .keys()
            .map(|node_id| (node_id.clone(), IndexSet::new()))
            .collect();
        for (from_node_id, blocked) in &self.blocking {
            for (to_node_id, sockets) in blocked {
                if sockets.values().any(|strong| *strong) {
                    if let Some(set) = blocked_by.get_mut(to_node_id) {
                        set.insert(from_node_id.clone());
                    }
                }
            }
        }
        let mut to_remove: Vec<String> = blocked_by
            .iter()
            .filter(|(_, blockers)| blockers.is_empty())
            .map(|(id, _)| id.clone())
            .collect();
        while !to_remove.is_empty() {
            for node_id in &to_remove {
                for blockers in blocked_by.values_mut() {
                    blockers.shift_remove(node_id);
                }
                blocked_by.shift_remove(node_id);
            }
            to_remove = blocked_by
                .iter()
                .filter(|(_, blockers)| blockers.is_empty())
                .map(|(id, _)| id.clone())
                .collect();
        }
        blocked_by.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn prompt_from_json(value: Value) -> DynamicPrompt {
        DynamicPrompt::new(value.as_object().unwrap().clone())
    }

    /// load -> decode -> save, plus an independent node.
    fn linear_prompt() -> DynamicPrompt {
        prompt_from_json(json!({
            "save": {"class_type": "SaveImage", "inputs": {"images": ["decode", 0]}},
            "decode": {"class_type": "VAEDecode", "inputs": {"samples": ["load", 0]}},
            "load": {"class_type": "LoadLatent", "inputs": {}},
            "island": {"class_type": "Note", "inputs": {}}
        }))
    }

    #[test]
    fn execution_order_respects_dependencies() {
        let prompt = linear_prompt();
        let resolver = NoopResolver;
        let mut sort = TopologicalSort::new(&prompt, &resolver);
        sort.add_node("save", false, None).unwrap();
        let order = sort.execution_order().unwrap();
        let pos = |id: &str| order.iter().position(|n| n == id).unwrap();
        assert!(pos("load") < pos("decode"));
        assert!(pos("decode") < pos("save"));
        // "island" was never requested, so it is not scheduled.
        assert_eq!(order.len(), 3);
    }

    #[test]
    fn cached_nodes_are_not_scheduled() {
        let prompt = linear_prompt();
        let resolver = NoopResolver;
        let mut sort = TopologicalSort::new(&prompt, &resolver);
        sort.set_cached("decode");
        sort.add_node("save", false, None).unwrap();
        let order = sort.execution_order().unwrap();
        assert_eq!(order, vec!["save".to_string()]);
    }

    #[test]
    fn cycle_is_detected_and_reported() {
        let prompt = prompt_from_json(json!({
            "a": {"class_type": "T", "inputs": {"in": ["c", 0]}},
            "b": {"class_type": "T", "inputs": {"in": ["a", 0]}},
            "c": {"class_type": "T", "inputs": {"in": ["b", 0]}},
            "root": {"class_type": "T", "inputs": {"in": ["c", 0]}}
        }));
        let resolver = NoopResolver;
        let mut sort = TopologicalSort::new(&prompt, &resolver);
        sort.add_node("root", false, None).unwrap();
        let err = sort.execution_order().unwrap_err();
        match err {
            GraphError::DependencyCycle { mut nodes_in_cycle } => {
                nodes_in_cycle.sort();
                // Like the Python implementation, nodes downstream of the
                // cycle ("root") are also reported: only sources dissolve.
                assert_eq!(nodes_in_cycle, vec!["a", "b", "c", "root"]);
            }
            other => panic!("expected cycle error, got {other:?}"),
        }
    }

    struct LazyIn;
    impl NodeDefResolver for LazyIn {
        fn is_input_lazy(&self, _class_type: &str, input_name: &str) -> bool {
            input_name == "lazy_in"
        }
    }

    #[test]
    fn lazy_inputs_are_skipped_unless_included() {
        let prompt = prompt_from_json(json!({
            "expensive": {"class_type": "T", "inputs": {}},
            "switch": {"class_type": "T", "inputs": {"lazy_in": ["expensive", 0]}}
        }));
        let resolver = LazyIn;
        let mut sort = TopologicalSort::new(&prompt, &resolver);
        sort.add_node("switch", false, None).unwrap();
        assert_eq!(sort.execution_order().unwrap(), vec!["switch".to_string()]);

        let mut sort = TopologicalSort::new(&prompt, &resolver);
        sort.add_node("switch", true, None).unwrap();
        let order = sort.execution_order().unwrap();
        assert_eq!(order, vec!["expensive".to_string(), "switch".to_string()]);
    }

    #[test]
    fn make_input_strong_link_errors() {
        let prompt = prompt_from_json(json!({
            "n": {"class_type": "T", "inputs": {"c": 5}}
        }));
        let resolver = NoopResolver;
        let mut sort = TopologicalSort::new(&prompt, &resolver);
        sort.add_node("n", false, None).unwrap();
        assert!(matches!(
            sort.make_input_strong_link("n", "missing"),
            Err(GraphError::NodeInput(_))
        ));
        assert!(matches!(
            sort.make_input_strong_link("n", "c"),
            Err(GraphError::NodeInput(_))
        ));
    }

    #[test]
    fn dynamic_prompt_ephemeral_lookup_chains() {
        let mut prompt = prompt_from_json(json!({
            "real": {"class_type": "T", "inputs": {}}
        }));
        prompt.add_ephemeral_node(
            "e1",
            json!({"class_type": "T", "inputs": {}}),
            "real",
            "real",
        );
        prompt.add_ephemeral_node("e2", json!({"class_type": "T", "inputs": {}}), "e1", "e1");
        assert_eq!(prompt.get_real_node_id("e2"), "real");
        assert_eq!(prompt.get_display_node_id("e2"), "real");
        assert_eq!(prompt.get_parent_node_id("e2"), Some("e1"));
        assert!(prompt.has_node("e2"));
        assert!(prompt.get_node("nope").is_err());
        assert_eq!(prompt.all_node_ids().len(), 3);
    }

    #[test]
    fn subgraph_filter_excludes_external_links() {
        let prompt = prompt_from_json(json!({
            "outside": {"class_type": "T", "inputs": {}},
            "inside": {"class_type": "T", "inputs": {"in": ["outside", 0]}}
        }));
        let resolver = NoopResolver;
        let mut sort = TopologicalSort::new(&prompt, &resolver);
        let subgraph: IndexSet<String> = ["inside".to_string()].into_iter().collect();
        sort.add_node("inside", false, Some(&subgraph)).unwrap();
        assert_eq!(sort.execution_order().unwrap(), vec!["inside".to_string()]);
    }
}
