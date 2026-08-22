use std::sync::Arc;

use modular_agent_core::{
    Agent, AgentContext, AgentData, AgentError, AgentOutput, AgentSpec, AgentValue, AsAgent,
    ModularAgent, async_trait, modular_agent,
};
use monty::MontyRun;
use monty_types::{CompileOptions, DictPairs, MontyObject, NoLimitTracker, PrintWriter};

static CATEGORY: &str = "Script/Monty";

static PORT_VALUE: &str = "value";

static CONFIG_SCRIPT: &str = "script";
static CONFIG_SKIP_UNIT: &str = "skip_unit";

/// Monty Script agent for executing Python-like scripts.
///
/// Uses [pydantic/monty](https://github.com/pydantic/monty), a Rust-native Python interpreter,
/// to run user-provided scripts with the input value as a parameter.
///
/// - Scripts receive input as the variable `value`
/// - The last expression's value becomes the output
/// - Scripts are compiled fresh on each invocation
///
/// # Configuration
/// - `script`: Python-like script to execute (text/multiline)
/// - `skip_unit`: When `true`, suppress output if the script returns `None`
///   (or `Ellipsis`). Explicit `return None` is also suppressed. (default: `false`)
///
/// # Ports
/// - Input `value`: Value passed to the script as the `value` variable
/// - Output `value`: Result of the last expression in the script
#[modular_agent(
    title = "Monty Script",
    category = CATEGORY,
    inputs = [PORT_VALUE],
    outputs = [PORT_VALUE],
    text_config(name = CONFIG_SCRIPT),
    boolean_config(name = CONFIG_SKIP_UNIT),
)]
struct MontyScriptAgent {
    data: AgentData,
}

#[async_trait]
impl AsAgent for MontyScriptAgent {
    fn new(ma: ModularAgent, id: String, spec: AgentSpec) -> Result<Self, AgentError> {
        Ok(Self {
            data: AgentData::new(ma, id, spec),
        })
    }

    async fn process(
        &mut self,
        ctx: AgentContext,
        _port: String,
        value: AgentValue,
    ) -> Result<(), AgentError> {
        let config = self.configs()?;
        let script = config.get_string(CONFIG_SCRIPT)?;
        if script.is_empty() {
            return Ok(());
        }

        let input = agent_value_to_monty_object(&value);
        let result = self
            .runtime()?
            .spawn_blocking(move || run_monty_script(script, input))
            .await
            .map_err(|e| AgentError::IoError(format!("Monty task error: {e}")))??;

        if matches!(result, AgentValue::Unit) && config.get_bool_or_default(CONFIG_SKIP_UNIT) {
            return Ok(());
        }

        self.output(ctx, PORT_VALUE, result).await
    }
}

fn run_monty_script(script: String, input: MontyObject) -> Result<AgentValue, AgentError> {
    let runner = MontyRun::new(
        script,
        "script.py",
        vec!["value".to_owned()],
        CompileOptions::default(),
    )
    .map_err(|e| AgentError::InvalidValue(format!("Monty compile error: {e}")))?;
    let result = runner
        .run(vec![input], NoLimitTracker, PrintWriter::Stdout)
        .map_err(|e| AgentError::InvalidValue(format!("Monty runtime error: {e}")))?;
    Ok(monty_object_to_agent_value(result))
}

fn agent_value_to_monty_object(value: &AgentValue) -> MontyObject {
    match value {
        AgentValue::Unit => MontyObject::None,
        AgentValue::Boolean(b) => MontyObject::Bool(*b),
        AgentValue::Integer(i) => MontyObject::Int(*i),
        AgentValue::Number(n) => MontyObject::Float(*n),
        AgentValue::String(s) => MontyObject::String(s.as_ref().clone()),
        AgentValue::Array(arr) => {
            MontyObject::List(arr.iter().map(agent_value_to_monty_object).collect())
        }
        AgentValue::Object(map) => {
            let pairs: Vec<(MontyObject, MontyObject)> = map
                .iter()
                .map(|(k, v)| {
                    (
                        MontyObject::String(k.clone()),
                        agent_value_to_monty_object(v),
                    )
                })
                .collect();
            MontyObject::Dict(DictPairs::from(pairs))
        }
        AgentValue::Tensor(t) => {
            MontyObject::List(t.iter().map(|f| MontyObject::Float(*f as f64)).collect())
        }
        AgentValue::Message(_) => json_value_to_monty_object(value.to_json()),
        AgentValue::Error(e) => MontyObject::String(format!("{e}")),
        AgentValue::Image(_) => MontyObject::None,
    }
}

/// Converts a `serde_json::Value` into a `MontyObject`, mirroring the mapping in
/// [`agent_value_to_monty_object`]. Used for `AgentValue` variants best represented
/// through their JSON form (currently `Message`), so scripts receive them as a
/// `dict` rather than a JSON string.
fn json_value_to_monty_object(value: serde_json::Value) -> MontyObject {
    match value {
        serde_json::Value::Null => MontyObject::None,
        serde_json::Value::Bool(b) => MontyObject::Bool(b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => MontyObject::Int(i),
            None => MontyObject::Float(n.as_f64().unwrap_or(0.0)),
        },
        serde_json::Value::String(s) => MontyObject::String(s),
        serde_json::Value::Array(arr) => {
            MontyObject::List(arr.into_iter().map(json_value_to_monty_object).collect())
        }
        serde_json::Value::Object(map) => {
            let pairs: Vec<(MontyObject, MontyObject)> = map
                .into_iter()
                .map(|(k, v)| (MontyObject::String(k), json_value_to_monty_object(v)))
                .collect();
            MontyObject::Dict(DictPairs::from(pairs))
        }
    }
}

fn monty_object_to_agent_value(obj: MontyObject) -> AgentValue {
    match obj {
        MontyObject::None | MontyObject::Ellipsis => AgentValue::Unit,
        MontyObject::Bool(b) => AgentValue::Boolean(b),
        MontyObject::Int(i) => AgentValue::Integer(i),
        MontyObject::BigInt(n) => {
            let s = n.to_string();
            match s.parse::<i64>() {
                Ok(i) => AgentValue::Integer(i),
                Err(_) => AgentValue::String(Arc::new(s)),
            }
        }
        MontyObject::Float(f) => AgentValue::Number(f),
        MontyObject::String(s) | MontyObject::Path(s) | MontyObject::Repr(s) => {
            AgentValue::String(Arc::new(s))
        }
        MontyObject::Bytes(b) => AgentValue::String(Arc::new(
            b.iter().map(|byte| format!("{byte:02x}")).collect(),
        )),
        MontyObject::List(items)
        | MontyObject::Tuple(items)
        | MontyObject::Set(items)
        | MontyObject::FrozenSet(items) => {
            AgentValue::Array(items.into_iter().map(monty_object_to_agent_value).collect())
        }
        MontyObject::Dict(pairs) => {
            let map: im::HashMap<String, AgentValue> = pairs
                .into_iter()
                .map(|(k, v)| (format!("{k}"), monty_object_to_agent_value(v)))
                .collect();
            AgentValue::Object(map)
        }
        MontyObject::NamedTuple {
            field_names,
            values,
            ..
        } => {
            let map: im::HashMap<String, AgentValue> = field_names
                .into_iter()
                .zip(values.into_iter().map(monty_object_to_agent_value))
                .collect();
            AgentValue::Object(map)
        }
        MontyObject::Dataclass {
            field_names, attrs, ..
        } => {
            let mut map: im::HashMap<String, AgentValue> = attrs
                .into_iter()
                .map(|(k, v)| (format!("{k}"), monty_object_to_agent_value(v)))
                .collect();
            let ordered: im::HashMap<String, AgentValue> = field_names
                .into_iter()
                .filter_map(|name| map.remove(&name).map(|v| (name, v)))
                .collect();
            AgentValue::Object(ordered)
        }
        MontyObject::Exception { exc_type, arg } => {
            let msg = match arg {
                Some(a) => format!("{exc_type}: {a}"),
                None => format!("{exc_type}"),
            };
            AgentValue::Error(Arc::new(AgentError::InvalidValue(msg)))
        }
        other => AgentValue::String(Arc::new(format!("{other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use modular_agent_core::{ContentBlock, Message, MessageContent};

    /// `MontyObject` has no `PartialEq`, so flatten a `DictPairs` into
    /// `(key_string, value)` pairs and match on variants.
    fn dict_fields(pairs: DictPairs) -> Vec<(String, MontyObject)> {
        pairs
            .into_iter()
            .map(|(k, v)| match k {
                MontyObject::String(s) => (s, v),
                other => (format!("{other}"), v),
            })
            .collect()
    }

    fn field<'a>(fields: &'a [(String, MontyObject)], key: &str) -> Option<&'a MontyObject> {
        fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn nth_dict(items: &[MontyObject], i: usize) -> Vec<(String, MontyObject)> {
        match items[i].clone() {
            MontyObject::Dict(pairs) => dict_fields(pairs),
            other => panic!("expected item {i} to be a Dict, got {other:?}"),
        }
    }

    #[test]
    fn message_input_becomes_dict() {
        let value = AgentValue::message(Message::user("hi".into()));
        let MontyObject::Dict(pairs) = agent_value_to_monty_object(&value) else {
            panic!("expected Message to convert to a Dict");
        };
        let fields = dict_fields(pairs);
        assert!(matches!(field(&fields, "role"), Some(MontyObject::String(s)) if s == "user"));
        assert!(matches!(field(&fields, "content"), Some(MontyObject::String(s)) if s == "hi"));
    }

    #[test]
    fn nested_message_in_object_becomes_dict() {
        let mut map = im::HashMap::new();
        map.insert(
            "message".to_string(),
            AgentValue::message(Message::user("hi".into())),
        );
        let MontyObject::Dict(pairs) = agent_value_to_monty_object(&AgentValue::Object(map)) else {
            panic!("expected Object to convert to a Dict");
        };
        let fields = dict_fields(pairs);
        let Some(MontyObject::Dict(inner)) = field(&fields, "message").cloned() else {
            panic!("expected nested message to be a Dict");
        };
        let inner = dict_fields(inner);
        assert!(matches!(field(&inner, "role"), Some(MontyObject::String(s)) if s == "user"));
        assert!(matches!(field(&inner, "content"), Some(MontyObject::String(s)) if s == "hi"));
    }

    #[test]
    fn message_with_block_content_becomes_list_of_dicts() {
        // A message carrying a non-text block (thinking) serializes `content`
        // as a tagged array, not a string, so scripts see `value["content"]`
        // as a list of block dicts.
        let msg = Message {
            role: "assistant".to_string(),
            content: MessageContent::Blocks(vec![
                ContentBlock::Thinking {
                    thinking: "reasoning".to_string(),
                    signature: None,
                    redacted: false,
                },
                ContentBlock::Text {
                    text: "hi".to_string(),
                },
            ]),
            ..Default::default()
        };
        let MontyObject::Dict(pairs) = agent_value_to_monty_object(&AgentValue::message(msg))
        else {
            panic!("expected Message to convert to a Dict");
        };
        let fields = dict_fields(pairs);
        let Some(MontyObject::List(blocks)) = field(&fields, "content").cloned() else {
            panic!("block content should convert to a List, not a JSON string");
        };
        assert_eq!(blocks.len(), 2);

        let thinking = nth_dict(&blocks, 0);
        assert!(
            matches!(field(&thinking, "type"), Some(MontyObject::String(s)) if s == "thinking")
        );
        assert!(
            matches!(field(&thinking, "thinking"), Some(MontyObject::String(s)) if s == "reasoning")
        );

        let text = nth_dict(&blocks, 1);
        assert!(matches!(field(&text, "type"), Some(MontyObject::String(s)) if s == "text"));
        assert!(matches!(field(&text, "text"), Some(MontyObject::String(s)) if s == "hi"));
    }

    #[test]
    fn json_number_maps_to_int_or_float() {
        assert!(matches!(
            json_value_to_monty_object(serde_json::json!(7)),
            MontyObject::Int(7)
        ));
        assert!(matches!(
            json_value_to_monty_object(serde_json::json!(2.5)),
            MontyObject::Float(f) if (f - 2.5).abs() < 1e-9
        ));
    }
}
