//! Validation and normalisation of `debug.json` configurations for netcoredbg.
//!
//! netcoredbg's launch/attach contract (from its VS Code protocol handler):
//! launch reads `program`, `args`, `cwd`, `env`, `stopAtEntry`, `justMyCode`,
//! `enableStepFiltering`; attach reads `processId`.

use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Launch,
    Attach,
}

pub fn request_kind(config: &Value) -> Result<Kind, String> {
    match config.get("request").and_then(Value::as_str) {
        Some("launch") => Ok(Kind::Launch),
        Some("attach") => Ok(Kind::Attach),
        Some(other) => Err(format!(
            "unsupported `request` {other:?}: expected \"launch\" or \"attach\""
        )),
        None => Err("missing `request`: set it to \"launch\" or \"attach\"".into()),
    }
}

fn scalar_to_string(value: &Value, what: &str) -> Result<String, String> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        _ => Err(format!("{what} must be a string, number or boolean")),
    }
}

fn optional_bool(obj: &Map<String, Value>, key: &str, default: bool) -> Result<bool, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(format!("`{key}` must be true or false")),
    }
}

fn parse_pid(value: &Value) -> Result<u32, String> {
    let pid = match value {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    };
    // netcoredbg parses the id into a signed 32-bit int, and pid 0 is never a real process.
    match pid {
        Some(p) if (1..=i32::MAX as u64).contains(&p) => Ok(p as u32),
        _ => Err("`processId` must be a positive integer process id".into()),
    }
}

/// Checks `config` and returns it with every optional field made explicit.
/// `default_cwd` is used when `cwd` is absent (the worktree root).
pub fn normalize(config: Value, default_cwd: &str) -> Result<(Kind, Value), String> {
    let kind = request_kind(&config)?;
    let Value::Object(mut obj) = config else {
        return Err("debug configuration must be a JSON object".into());
    };

    match kind {
        Kind::Attach => {
            let pid = obj
                .get("processId")
                .ok_or("attach requires `processId`")
                .map_err(String::from)
                .and_then(parse_pid)?;
            obj.insert("processId".into(), Value::from(pid));
        }
        Kind::Launch => {
            let program = match obj.get("program") {
                Some(Value::String(s)) if !s.trim().is_empty() && !s.contains('\0') => s.clone(),
                Some(_) => return Err("`program` must be a non-empty string".into()),
                None => {
                    return Err(
                        "launch requires `program`: the path to your built .dll (or apphost executable)".into(),
                    )
                }
            };
            obj.insert("program".into(), Value::String(program));

            let args = match obj.get("args") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|v| scalar_to_string(v, "each entry of `args`"))
                    .collect::<Result<Vec<_>, _>>()?,
                Some(Value::String(_)) => {
                    return Err("`args` must be an array of strings, not a single string".into())
                }
                Some(_) => return Err("`args` must be an array of strings".into()),
            };
            obj.insert("args".into(), Value::from(args));

            let cwd = match obj.get("cwd") {
                None | Some(Value::Null) => default_cwd.to_string(),
                Some(Value::String(s)) if !s.is_empty() && !s.contains('\0') => s.clone(),
                Some(_) => return Err("`cwd` must be a non-empty string".into()),
            };
            obj.insert("cwd".into(), Value::String(cwd));

            let mut env = Map::new();
            match obj.get("env") {
                None | Some(Value::Null) => {}
                Some(Value::Object(vars)) => {
                    for (key, value) in vars {
                        // `=` or NUL in a name would be misparsed by the OS environment block.
                        if key.is_empty() || key.contains('=') || key.contains('\0') {
                            return Err(format!("invalid environment variable name {key:?}"));
                        }
                        if value.is_null() {
                            continue;
                        }
                        let text = scalar_to_string(value, &format!("env.{key}"))?;
                        if text.contains('\0') {
                            return Err(format!("env.{key} contains a NUL byte"));
                        }
                        env.insert(key.clone(), Value::String(text));
                    }
                }
                Some(_) => return Err("`env` must be an object of name/value pairs".into()),
            }
            obj.insert("env".into(), Value::Object(env));

            // Zed's generic UI calls this `stopOnEntry`; netcoredbg calls it `stopAtEntry`.
            let stop_default = optional_bool(&obj, "stopOnEntry", false)?;
            let stop = optional_bool(&obj, "stopAtEntry", stop_default)?;
            obj.insert("stopAtEntry".into(), Value::Bool(stop));
            let just_my_code = optional_bool(&obj, "justMyCode", true)?;
            obj.insert("justMyCode".into(), Value::Bool(just_my_code));
            let step_filtering = optional_bool(&obj, "enableStepFiltering", true)?;
            obj.insert("enableStepFiltering".into(), Value::Bool(step_filtering));
        }
    }
    Ok((kind, Value::Object(obj)))
}

pub struct LaunchParams {
    pub program: String,
    pub cwd: Option<String>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub stop_on_entry: Option<bool>,
}

/// Builds a `debug.json` style config from Zed's generic "new debug session" fields.
pub fn launch_config(p: &LaunchParams) -> Value {
    let mut obj = Map::new();
    obj.insert("request".into(), "launch".into());
    obj.insert("program".into(), p.program.clone().into());
    obj.insert("args".into(), Value::from(p.args.clone()));
    if let Some(cwd) = &p.cwd {
        obj.insert("cwd".into(), cwd.clone().into());
    }
    let env: Map<String, Value> = p
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone().into()))
        .collect();
    obj.insert("env".into(), Value::Object(env));
    if let Some(stop) = p.stop_on_entry {
        obj.insert("stopAtEntry".into(), stop.into());
    }
    Value::Object(obj)
}

pub fn attach_config(process_id: Option<u32>) -> Result<Value, String> {
    let pid = process_id.ok_or("attach requires a process id")?;
    parse_pid(&Value::from(pid))?;
    Ok(serde_json::json!({ "request": "attach", "processId": pid }))
}

/// A `.csproj` given as the "program" means "build this project, then debug its output".
pub fn is_project_file(program: &str) -> bool {
    let lower = program.to_ascii_lowercase();
    [".csproj", ".fsproj", ".vbproj"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_kind_requires_a_known_value() {
        assert_eq!(
            request_kind(&json!({"request":"launch"})).unwrap(),
            Kind::Launch
        );
        assert_eq!(
            request_kind(&json!({"request":"attach"})).unwrap(),
            Kind::Attach
        );
        assert!(request_kind(&json!({"request":"nope"})).is_err());
        assert!(request_kind(&json!({})).is_err());
    }

    #[test]
    fn launch_defaults_are_made_explicit() {
        let (kind, v) =
            normalize(json!({"request":"launch","program":"bin/App.dll"}), "/work").unwrap();
        assert_eq!(kind, Kind::Launch);
        assert_eq!(v["cwd"], "/work");
        assert_eq!(v["args"], json!([]));
        assert_eq!(v["env"], json!({}));
        assert_eq!(v["stopAtEntry"], false);
        assert_eq!(v["justMyCode"], true);
        assert_eq!(v["enableStepFiltering"], true);
    }

    #[test]
    fn launch_coerces_scalars_and_honours_stop_on_entry_alias() {
        let (_, v) = normalize(
            json!({"request":"launch","program":"a.dll","args":["x",1,true],
                   "env":{"A":1,"B":null,"C":"c"},"stopOnEntry":true,"justMyCode":false}),
            "/w",
        )
        .unwrap();
        assert_eq!(v["args"], json!(["x", "1", "true"]));
        assert_eq!(v["env"], json!({"A":"1","C":"c"}));
        assert_eq!(v["stopAtEntry"], true);
        assert_eq!(v["justMyCode"], false);
    }

    #[test]
    fn launch_rejects_bad_input_with_actionable_messages() {
        let bad = |v: Value| normalize(v, "/w").unwrap_err();
        assert!(bad(json!({"request":"launch"})).contains("program"));
        assert!(bad(json!({"request":"launch","program":" "})).contains("program"));
        assert!(bad(json!({"request":"launch","program":"a","args":"x y"})).contains("array"));
        assert!(bad(json!({"request":"launch","program":"a","env":{"A=B":"1"}})).contains("name"));
        assert!(
            bad(json!({"request":"launch","program":"a","justMyCode":"yes"}))
                .contains("justMyCode")
        );
        assert!(bad(json!({"request":"launch","program":"a\u{0}b"})).contains("program"));
    }

    #[test]
    fn attach_accepts_numeric_and_string_pids_only_in_range() {
        for pid in [json!(42), json!("42")] {
            let (_, v) = normalize(json!({"request":"attach","processId":pid}), "/w").unwrap();
            assert_eq!(v["processId"], 42);
        }
        for pid in [
            json!(0),
            json!(-1),
            json!("abc"),
            json!(4294967295u64),
            json!(null),
        ] {
            assert!(normalize(json!({"request":"attach","processId":pid}), "/w").is_err());
        }
        assert!(normalize(json!({"request":"attach"}), "/w").is_err());
    }

    #[test]
    fn generic_launch_maps_to_config_that_normalizes() {
        let cfg = launch_config(&LaunchParams {
            program: "a.dll".into(),
            cwd: None,
            args: vec!["--x".into()],
            env: vec![("K".into(), "V".into())],
            stop_on_entry: Some(true),
        });
        let (_, v) = normalize(cfg, "/w").unwrap();
        assert_eq!(v["env"], json!({"K":"V"}));
        assert_eq!(v["stopAtEntry"], true);
        assert!(attach_config(None).is_err());
        assert_eq!(attach_config(Some(7)).unwrap()["processId"], 7);
    }

    #[test]
    fn project_files_are_recognised() {
        assert!(is_project_file("src/App/App.CSPROJ"));
        assert!(is_project_file("a.fsproj"));
        assert!(!is_project_file("bin/App.dll"));
    }
}
