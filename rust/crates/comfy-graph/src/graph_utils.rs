//! Port of `comfy_execution/graph_utils.py`.

use std::sync::Mutex;

use indexmap::IndexMap;
use serde_json::{json, Map, Value};

/// A link is a JSON array of the form `[node_id, socket_index]` where the
/// first element is a string and the second is a number. This matches the
/// Python `is_link` check, which accepts both ints and floats for the socket.
pub fn is_link(value: &Value) -> bool {
    match value.as_array() {
        Some(arr) => {
            arr.len() == 2
                && arr[0].is_string()
                && (arr[1].is_i64() || arr[1].is_u64() || arr[1].is_f64())
        }
        None => false,
    }
}

/// Strongly typed view of a link value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Link {
    pub node_id: String,
    pub socket: u32,
}

impl Link {
    /// Parse a JSON value as a link. Returns `None` when the value is not a
    /// link or the socket index is not a non-negative integer.
    pub fn parse(value: &Value) -> Option<Link> {
        if !is_link(value) {
            return None;
        }
        let arr = value.as_array().unwrap();
        let socket = arr[1]
            .as_u64()
            .or_else(|| arr[1].as_f64().map(|f| f as u64))?;
        Some(Link {
            node_id: arr[0].as_str().unwrap().to_string(),
            socket: socket as u32,
        })
    }

    pub fn to_value(&self) -> Value {
        json!([self.node_id, self.socket])
    }
}

/// Return this from a node to block any consumers with the given error
/// message. A `None` message blocks execution silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionBlocker {
    pub message: Option<String>,
}

impl ExecutionBlocker {
    pub fn new(message: Option<String>) -> Self {
        Self { message }
    }
}

/// A node under construction inside a [`GraphBuilder`].
#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    pub class_type: String,
    pub inputs: Map<String, Value>,
    pub override_display_id: Option<String>,
}

impl Node {
    pub fn new(
        id: impl Into<String>,
        class_type: impl Into<String>,
        inputs: Map<String, Value>,
    ) -> Self {
        Self {
            id: id.into(),
            class_type: class_type.into(),
            inputs,
            override_display_id: None,
        }
    }

    /// A link to the given output socket of this node.
    pub fn out(&self, index: u32) -> Value {
        json!([self.id, index])
    }

    /// Set an input; a `Value::Null` removes the input, matching the Python
    /// behavior where `None` deletes the key.
    pub fn set_input(&mut self, key: &str, value: Value) {
        if value.is_null() {
            self.inputs.shift_remove(key);
        } else {
            self.inputs.insert(key.to_string(), value);
        }
    }

    pub fn get_input(&self, key: &str) -> Option<&Value> {
        self.inputs.get(key)
    }

    pub fn set_override_display_id(&mut self, override_display_id: impl Into<String>) {
        self.override_display_id = Some(override_display_id.into());
    }

    pub fn serialize(&self) -> Value {
        let mut serialized = Map::new();
        serialized.insert(
            "class_type".to_string(),
            Value::String(self.class_type.clone()),
        );
        serialized.insert("inputs".to_string(), Value::Object(self.inputs.clone()));
        if let Some(display_id) = &self.override_display_id {
            serialized.insert(
                "override_display_id".to_string(),
                Value::String(display_id.clone()),
            );
        }
        Value::Object(serialized)
    }
}

#[derive(Debug, Clone, Default)]
struct DefaultPrefix {
    root: String,
    call_index: u64,
    graph_index: u64,
}

static DEFAULT_PREFIX: Mutex<DefaultPrefix> = Mutex::new(DefaultPrefix {
    root: String::new(),
    call_index: 0,
    graph_index: 0,
});

/// Utility that outputs graphs in the JSON form expected by the ComfyUI
/// backend. Port of the Python `GraphBuilder`.
#[derive(Debug)]
pub struct GraphBuilder {
    pub prefix: String,
    nodes: IndexMap<String, Node>,
    id_gen: u64,
}

impl GraphBuilder {
    pub fn new(prefix: Option<String>) -> Self {
        let prefix = prefix.unwrap_or_else(|| Self::alloc_prefix(None, None, None));
        Self {
            prefix,
            nodes: IndexMap::new(),
            id_gen: 1,
        }
    }

    pub fn set_default_prefix(prefix_root: &str, call_index: u64, graph_index: u64) {
        let mut default = DEFAULT_PREFIX.lock().unwrap();
        default.root = prefix_root.to_string();
        default.call_index = call_index;
        default.graph_index = graph_index;
    }

    pub fn alloc_prefix(
        root: Option<&str>,
        call_index: Option<u64>,
        graph_index: Option<u64>,
    ) -> String {
        let mut default = DEFAULT_PREFIX.lock().unwrap();
        let root = root.unwrap_or(&default.root);
        let call_index = call_index.unwrap_or(default.call_index);
        let graph_index = graph_index.unwrap_or(default.graph_index);
        let result = format!("{root}.{call_index}.{graph_index}.");
        default.graph_index += 1;
        result
    }

    /// Create (or return the existing) node with the given class type. When
    /// `id` is `None` a sequential id is allocated.
    pub fn node(
        &mut self,
        class_type: &str,
        id: Option<&str>,
        inputs: Map<String, Value>,
    ) -> &mut Node {
        let id = match id {
            Some(id) => id.to_string(),
            None => {
                let id = self.id_gen.to_string();
                self.id_gen += 1;
                id
            }
        };
        let id = format!("{}{}", self.prefix, id);
        if !self.nodes.contains_key(&id) {
            let node = Node::new(id.clone(), class_type, inputs);
            self.nodes.insert(id.clone(), node);
        }
        self.nodes.get_mut(&id).unwrap()
    }

    pub fn lookup_node(&mut self, id: &str) -> Option<&mut Node> {
        let id = format!("{}{}", self.prefix, id);
        self.nodes.get_mut(&id)
    }

    /// Serialize all nodes to the ComfyUI API prompt format.
    pub fn finalize(&self) -> Map<String, Value> {
        let mut output = Map::new();
        for (node_id, node) in &self.nodes {
            output.insert(node_id.clone(), node.serialize());
        }
        output
    }

    /// Rewrite every input that links to `(node_id, index)`. A `Value::Null`
    /// replacement removes the input entirely.
    pub fn replace_node_output(&mut self, node_id: &str, index: u32, new_value: Value) {
        let node_id = format!("{}{}", self.prefix, node_id);
        let target = json!([node_id, index]);
        for node in self.nodes.values_mut() {
            let keys_to_remove: Vec<String> = node
                .inputs
                .iter()
                .filter(|(_, value)| is_link(value) && links_match(value, &target))
                .map(|(key, _)| key.clone())
                .collect();
            for key in keys_to_remove {
                if new_value.is_null() {
                    node.inputs.shift_remove(&key);
                } else {
                    node.inputs.insert(key, new_value.clone());
                }
            }
        }
    }

    pub fn remove_node(&mut self, id: &str) {
        let id = format!("{}{}", self.prefix, id);
        self.nodes.shift_remove(&id);
    }
}

fn links_match(a: &Value, b: &Value) -> bool {
    match (Link::parse(a), Link::parse(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Prefix every node id (and any internal links) in `graph`, and remap the
/// given outputs. Port of the Python `add_graph_prefix`.
pub fn add_graph_prefix(
    graph: &Map<String, Value>,
    outputs: &[Value],
    prefix: &str,
) -> (Map<String, Value>, Vec<Value>) {
    let mut new_graph = Map::new();
    for (node_id, node_info) in graph {
        let new_node_id = format!("{prefix}{node_id}");
        let class_type = node_info.get("class_type").cloned().unwrap_or(Value::Null);
        let mut new_inputs = Map::new();
        if let Some(inputs) = node_info.get("inputs").and_then(|v| v.as_object()) {
            for (input_name, input_value) in inputs {
                if is_link(input_value) {
                    let arr = input_value.as_array().unwrap();
                    let from_id = arr[0].as_str().unwrap();
                    new_inputs.insert(
                        input_name.clone(),
                        json!([format!("{prefix}{from_id}"), arr[1]]),
                    );
                } else {
                    new_inputs.insert(input_name.clone(), input_value.clone());
                }
            }
        }
        let mut new_node = Map::new();
        new_node.insert("class_type".to_string(), class_type);
        new_node.insert("inputs".to_string(), Value::Object(new_inputs));
        new_graph.insert(new_node_id, Value::Object(new_node));
    }

    let new_outputs = outputs
        .iter()
        .map(|output| {
            if is_link(output) {
                let arr = output.as_array().unwrap();
                let from_id = arr[0].as_str().unwrap();
                json!([format!("{prefix}{from_id}"), arr[1]])
            } else {
                output.clone()
            }
        })
        .collect();

    (new_graph, new_outputs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_link_matches_python_semantics() {
        assert!(is_link(&json!(["node1", 0])));
        assert!(is_link(&json!(["node1", 0.0])));
        assert!(!is_link(&json!("node1")));
        assert!(!is_link(&json!(["node1"])));
        assert!(!is_link(&json!(["node1", 0, 1])));
        assert!(!is_link(&json!([0, "node1"])));
        assert!(!is_link(&json!(["node1", "0"])));
        assert!(!is_link(&json!(null)));
        assert!(!is_link(&json!({"a": 1})));
    }

    #[test]
    fn graph_builder_allocates_sequential_ids() {
        let mut builder = GraphBuilder::new(Some("test.0.0.".to_string()));
        let id1 = builder.node("LoadImage", None, Map::new()).id.clone();
        let id2 = builder.node("SaveImage", None, Map::new()).id.clone();
        assert_eq!(id1, "test.0.0.1");
        assert_eq!(id2, "test.0.0.2");
    }

    #[test]
    fn graph_builder_finalize_serializes_api_format() {
        let mut builder = GraphBuilder::new(Some("p.".to_string()));
        let mut inputs = Map::new();
        inputs.insert("image".to_string(), json!("example.png"));
        builder.node("LoadImage", Some("load"), inputs);
        let load_out = builder.lookup_node("load").unwrap().out(0);
        let mut save_inputs = Map::new();
        save_inputs.insert("images".to_string(), load_out);
        builder.node("SaveImage", Some("save"), save_inputs);

        let output = builder.finalize();
        assert_eq!(output["p.load"]["class_type"], json!("LoadImage"));
        assert_eq!(output["p.save"]["inputs"]["images"], json!(["p.load", 0]));
    }

    #[test]
    fn replace_node_output_rewrites_and_removes_links() {
        let mut builder = GraphBuilder::new(Some("p.".to_string()));
        builder.node("A", Some("a"), Map::new());
        let a_out = builder.lookup_node("a").unwrap().out(0);
        let mut b_inputs = Map::new();
        b_inputs.insert("x".to_string(), a_out.clone());
        b_inputs.insert("y".to_string(), a_out);
        builder.node("B", Some("b"), b_inputs);

        builder.replace_node_output("a", 0, json!(42));
        {
            let b = builder.lookup_node("b").unwrap();
            assert_eq!(b.inputs["x"], json!(42));
            assert_eq!(b.inputs["y"], json!(42));
        }

        // Null removes the inputs entirely.
        let mut builder = GraphBuilder::new(Some("q.".to_string()));
        builder.node("A", Some("a"), Map::new());
        let a_out = builder.lookup_node("a").unwrap().out(1);
        let mut b_inputs = Map::new();
        b_inputs.insert("x".to_string(), a_out);
        builder.node("B", Some("b"), b_inputs);
        builder.replace_node_output("a", 1, Value::Null);
        let b = builder.lookup_node("b").unwrap();
        assert!(b.inputs.get("x").is_none());
    }

    #[test]
    fn add_graph_prefix_remaps_internal_links_and_outputs() {
        let mut graph = Map::new();
        graph.insert("1".to_string(), json!({"class_type": "A", "inputs": {}}));
        graph.insert(
            "2".to_string(),
            json!({"class_type": "B", "inputs": {"in": ["1", 0], "k": 5}}),
        );
        let outputs = vec![json!(["2", 0]), json!("constant")];

        let (new_graph, new_outputs) = add_graph_prefix(&graph, &outputs, "sub.");
        assert!(new_graph.contains_key("sub.1"));
        assert_eq!(new_graph["sub.2"]["inputs"]["in"], json!(["sub.1", 0]));
        assert_eq!(new_graph["sub.2"]["inputs"]["k"], json!(5));
        assert_eq!(new_outputs[0], json!(["sub.2", 0]));
        assert_eq!(new_outputs[1], json!("constant"));
    }

    #[test]
    fn node_set_input_null_removes_key() {
        let mut node = Node::new("n", "T", Map::new());
        node.set_input("a", json!(1));
        assert_eq!(node.get_input("a"), Some(&json!(1)));
        node.set_input("a", Value::Null);
        assert_eq!(node.get_input("a"), None);
    }
}
