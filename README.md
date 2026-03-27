# Monty Agents for Modular Agent

Execute Python-like scripts in Modular Agent workflows using [pydantic/monty](https://github.com/pydantic/monty), a Rust-native Python interpreter.

## Features

- **Monty Script** — Run Python-like scripts to transform, filter, and reshape data in agent workflows without leaving the Rust runtime

## Installation

Two changes to add this package to [`modular-agent-desktop`](https://github.com/modular-agent/modular-agent-desktop):

1. **`modular-agent-desktop/src-tauri/Cargo.toml`** — add dependency:

   ```toml
   modular-agent-monty = { path = "../../modular-agent-monty" }
   ```

2. **`modular-agent-desktop/src-tauri/src/lib.rs`** — add import:

   ```rust
   #[allow(unused_imports)]
   use modular_agent_monty;
   ```

## Build Requirements

Monty depends on `pyo3-build-config`, which requires a Python interpreter at build time. Set the `PYO3_PYTHON` environment variable before building:

```bash
PYO3_PYTHON="py" cargo build --manifest-path modular-agent-monty/Cargo.toml
```

## Monty Script

Executes user-provided scripts through [pydantic/monty](https://github.com/pydantic/monty), a Rust-native Python interpreter that supports a subset of Python — not full CPython. See the [monty repository](https://github.com/pydantic/monty) for supported language features.

Scripts receive input as the variable `value` and the last expression's value becomes the output. Scripts are compiled fresh on each invocation. `print()` calls write to process stdout, not to output ports.

### Configuration

| Config | Type | Default | Description |
| ------ | ---- | ------- | ----------- |
| script | text | "" | Python-like script to execute. Empty script produces no output (silent no-op) |
| skip_unit | boolean | false | When true, suppress output if the script returns `None` or `Ellipsis` |

### Ports

- **Input**: `value` — Value passed to the script as the `value` variable
- **Output**: `value` — Result of the last expression in the script

### Usage Example

Double the input value:

```python
value * 2
```

If input is `5`, output is `10`.

Filter and transform a list:

```python
[x.upper() for x in value if len(x) > 3]
```

If input is `["hi", "hello", "hey", "world"]`, output is `["HELLO", "WORLD"]`.

Filter with `skip_unit` — set `skip_unit` to `true` and use `None` to suppress output:

```python
value if value > 0 else None
```

If input is `5`, output is `5`. If input is `-3`, no output is emitted.

### Type Mapping

**Input (AgentValue → Python):**

| AgentValue | Python Type | Notes |
| ---------- | ----------- | ----- |
| Unit | None | |
| Boolean | bool | |
| Integer | int | |
| Number | float | |
| String | str | |
| Array | list | Elements converted recursively |
| Object | dict | String keys, values converted recursively |
| Tensor | list | f32 values widened to float (f64) |
| Message | str | JSON-serialized |
| Error | str | Formatted error string |
| Image | None | Image data is not accessible from scripts |

**Output (Python → AgentValue):**

| Python Type | AgentValue | Notes |
| ----------- | ---------- | ----- |
| None, Ellipsis | Unit | Suppressed when `skip_unit` is true |
| bool | Boolean | |
| int | Integer | |
| big int | Integer or String | Integer if fits i64, otherwise string representation |
| float | Number | |
| str, Path, Repr | String | |
| bytes | String | Hex-encoded (e.g. `b"\xff"` → `"ff"`) |
| list, tuple, set, frozenset | Array | Elements converted recursively |
| dict | Object | Keys converted via Display |
| NamedTuple | Object | Field names as keys |
| dataclass | Object | Field-ordered keys |
| Exception | Error | Flows through output port, not a process failure |
| Other | String | Converted via Display |

### Limitations

Monty is a Rust-native Python interpreter that supports a subset of Python:

- No `import` statements — no standard library or third-party module access
- No file I/O or network access
- `print()` output goes to process stdout, not to any agent port

See the [monty repository](https://github.com/pydantic/monty) for the full list of supported language features.

### Error Handling

- **Compile errors** (invalid syntax) and **runtime errors** (e.g. `NameError`) are raised as `AgentError` and cause the agent's `process()` to fail.
- **Python exceptions** returned as values (e.g. from a caught exception stored in a variable) are converted to `AgentValue::Error` and flow through the output port normally.

## Architecture

- **Monty Script**: Synchronous execution via `spawn_blocking` to avoid blocking the async runtime. Fresh compilation per invocation (no bytecode cache). Bidirectional conversion between `AgentValue` and `MontyObject`.

## Key Dependencies

- [monty](https://github.com/pydantic/monty) — Rust-native Python interpreter (git dependency; crates.io has only a placeholder)

## License

Apache-2.0 OR MIT
