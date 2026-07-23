---
name: rust-serde-tolerant-parsing
description: Use when deserializing JSON from an external tool (herdr CLI output, APIs) whose shape you don't fully control. Gives this repo's tolerant-serde patterns — envelope unwrapping, rename_all, #[serde(other)], #[serde(default)].
---

# Tolerant serde parsing (herdr-pets conventions)

## When to use

Parsing anything herdr (or any external process) emits. The producer may add
fields, add enum variants, or omit optionals over time — your types must not
panic when it does. See `src/agent.rs`.

## The patterns

**1. Unwrap the envelope with throwaway structs.** herdr wraps payloads in
`{"result": {...}}`. Model the wrapper privately, return the inner value:

```rust
#[derive(Debug, Deserialize)]
struct Envelope { result: EnvelopeResult }
#[derive(Debug, Deserialize)]
struct EnvelopeResult { agents: Vec<Agent> }

pub fn parse_agent_list(json: &str) -> Result<Vec<Agent>, serde_json::Error> {
    let env: Envelope = serde_json::from_str(json)?;
    Ok(env.result.agents)
}
```

**2. Match the wire's casing with `rename_all`,** not by renaming every field:

```rust
#[serde(rename_all = "lowercase")]
enum AgentStatus { Idle, Working, Blocked, Done, /* ... */ }
```

**3. Survive unknown enum variants with `#[serde(other)]`.** A status string
herdr adds later deserializes to a fallback instead of erroring:

```rust
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Idle, Working, Blocked, Done,
    #[serde(other)]
    Unknown,        // any unrecognised string lands here
}
```

**4. Make genuinely-optional fields `Option<T>` with `#[serde(default)]`.**
Fields absent for some records (a pane with no detected agent) must not fail
the whole parse:

```rust
pub struct Agent {
    #[serde(default)] pub agent: Option<String>,
    #[serde(default)] pub name: Option<String>,
    pub agent_status: AgentStatus,   // required fields stay non-optional
    // ...
}
```

## Rules

- **Tolerance is a property of the type, not the caller.** Encode it with serde
  attributes so every call site benefits and no one has to remember to handle a
  missing field.
- **Only optional what's truly optional.** Keep required fields non-`Option` so a
  malformed record still fails loudly at parse time.
- **Keep envelope structs private** (`struct`, not `pub struct`); expose only the
  useful inner type.
- **Test the tolerance.** Add a case for an unrecognised variant and for absent
  optionals (`unrecognised_status_falls_back_to_unknown`,
  `optional_agent_and_name_are_none_when_absent`).

## Anti-patterns

- A giant flat struct with every field required — one new field from herdr and
  every parse fails.
- Post-processing casing in code (`s.to_lowercase()`) instead of `rename_all`.
- `Option` on fields that are always present, hiding real malformed-input bugs
  behind a silent `None`.
