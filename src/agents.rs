use std::sync::Arc;

use modular_agent_core::{
    Agent, AgentContext, AgentData, AgentError, AgentOutput, AgentSpec, AgentValue, AsAgent,
    ModularAgent, async_trait, modular_agent,
};
use monty::{DictPairs, MontyObject, MontyRun, NoLimitTracker, PrintWriter};

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
            .runtime()
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
    let runner = MontyRun::new(script, "script.py", vec!["value".to_owned()])
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
        AgentValue::Message(m) => {
            let json = serde_json::to_string(m.as_ref()).unwrap_or_default();
            MontyObject::String(json)
        }
        AgentValue::Error(e) => MontyObject::String(format!("{e}")),
        AgentValue::Image(_) => MontyObject::None,
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
