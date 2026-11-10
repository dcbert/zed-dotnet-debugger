//! Turns a `dotnet run` task into "build, then debug the built assembly".
//!
//! Phase 1 (`parse_dotnet_run` + `build_args`) rewrites the task into a `dotnet build`.
//! Phase 2 asks MSBuild where that build put its output (`msbuild_query_args` +
//! `parse_msbuild_properties`), so no `bin/Debug/netX.Y` path is ever guessed.

use serde_json::Value;

use crate::pinned::Os;

/// Environment variable carrying the program arguments from phase 1 to phase 2
/// (the build task is the only thing Zed hands back to us in between).
pub const ENV_RUN_ARGS: &str = "ZED_DOTNET_DEBUG_ARGS";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RunSpec {
    pub project: Option<String>,
    pub configuration: Option<String>,
    pub framework: Option<String>,
    pub env: Vec<(String, String)>,
    pub program_args: Vec<String>,
}

/// `None` when this is not a `dotnet run` we can turn into a debug session.
pub fn parse_dotnet_run(args: &[String]) -> Option<RunSpec> {
    if args.first().map(String::as_str) != Some("run") {
        return None;
    }
    let mut spec = RunSpec::default();
    let mut i = 1;
    while i < args.len() {
        let token = args[i].as_str();
        if token == "--" {
            spec.program_args = args[i + 1..].to_vec();
            break;
        }
        if !token.starts_with('-') {
            // File-based apps (`dotnet run app.cs`) have no project to build.
            if token.ends_with(".cs") {
                return None;
            }
            spec.program_args = args[i..].to_vec();
            break;
        }
        // MSBuild-style property switches (`-p:Foo=1`, `--property:Foo=1`) are not values of `-p`.
        if token.starts_with("-p:") || token.starts_with("--property:") {
            i += 1;
            continue;
        }
        let (flag, inline) = match token.split_once('=') {
            Some((f, v)) => (f, Some(v.to_string())),
            None => (token, None),
        };
        let takes_value = matches!(
            flag,
            "--project" | "-p" | "--configuration" | "-c" | "--framework" | "-f" | "--launch-profile"
                | "-lp" | "--runtime" | "-r" | "--arch" | "-a" | "--os" | "--verbosity" | "-v"
                | "--artifacts-path" | "--environment" | "-e"
        );
        if !takes_value {
            i += 1;
            continue;
        }
        let value = match inline {
            Some(v) => v,
            None => {
                i += 1;
                args.get(i)?.clone()
            }
        };
        match flag {
            "--project" | "-p" => spec.project = Some(value),
            "--configuration" | "-c" => spec.configuration = Some(value),
            "--framework" | "-f" => spec.framework = Some(value),
            "--environment" | "-e" => {
                let (k, v) = value.split_once('=')?;
                spec.env.push((k.to_string(), v.to_string()));
            }
            _ => {}
        }
        i += 1;
    }
    Some(spec)
}

pub fn build_args(spec: &RunSpec) -> Vec<String> {
    let mut args = vec!["build".to_string()];
    if let Some(p) = &spec.project {
        args.push(p.clone());
    }
    if let Some(c) = &spec.configuration {
        args.extend(["-c".into(), c.clone()]);
    }
    if let Some(f) = &spec.framework {
        args.extend(["-f".into(), f.clone()]);
    }
    args
}

/// Inverse of `build_args`, applied to the build task Zed hands back in phase 2.
pub fn parse_build_args(args: &[String]) -> RunSpec {
    let mut spec = RunSpec::default();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-c" | "--configuration" => {
                i += 1;
                spec.configuration = args.get(i).cloned();
            }
            "-f" | "--framework" => {
                i += 1;
                spec.framework = args.get(i).cloned();
            }
            t if !t.starts_with('-') && spec.project.is_none() => spec.project = Some(t.to_string()),
            _ => {}
        }
        i += 1;
    }
    spec
}

pub fn encode_program_args(args: &[String]) -> String {
    Value::from(args.to_vec()).to_string()
}

pub fn decode_program_args(raw: &str) -> Result<Vec<String>, String> {
    serde_json::from_str::<Vec<String>>(raw).map_err(|e| format!("corrupt {ENV_RUN_ARGS}: {e}"))
}

fn looks_like_project_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".csproj", ".fsproj", ".vbproj", ".sln", ".slnx", ".proj"].iter().any(|e| lower.ends_with(e))
}

fn is_absolute(path: &str) -> bool {
    path.starts_with('/') || path.starts_with('\\') || path.as_bytes().get(1) == Some(&b':')
}

fn join(base: &str, rel: &str) -> String {
    let sep = if base.contains('\\') && !base.contains('/') { '\\' } else { '/' };
    format!("{}{sep}{}", base.trim_end_matches(['/', '\\']), rel.trim_start_matches("./"))
}

pub fn dirname(path: &str) -> Option<&str> {
    let idx = path.rfind(['/', '\\'])?;
    if idx == 0 { Some(&path[..1]) } else { Some(&path[..idx]) }
}

/// Where MSBuild must run and which project file to hand it. A `--project` that is a
/// directory (accepted by `dotnet run`) becomes the working directory instead.
pub fn resolve_project(task_cwd: Option<&str>, project: Option<&str>) -> (Option<String>, Option<String>) {
    match project {
        Some(p) if looks_like_project_file(p) => (task_cwd.map(String::from), Some(p.to_string())),
        Some(dir) => {
            let full = match task_cwd {
                Some(c) if !is_absolute(dir) => join(c, dir),
                _ => dir.to_string(),
            };
            (Some(full), None)
        }
        None => (task_cwd.map(String::from), None),
    }
}

/// Working directory of the debuggee: the task's own, else the project's folder.
pub fn debuggee_cwd(task_cwd: Option<&str>, project_file: Option<&str>) -> Option<String> {
    if let Some(c) = task_cwd {
        return Some(c.to_string());
    }
    project_file.and_then(dirname).map(String::from)
}

pub fn msbuild_query_args(project_file: Option<&str>, spec: &RunSpec, framework_override: Option<&str>) -> Vec<String> {
    let mut args = vec!["msbuild".to_string()];
    if let Some(p) = project_file {
        args.push(p.to_string());
    }
    args.push("-nologo".into());
    for prop in ["TargetPath", "OutputType", "TargetFramework", "TargetFrameworks"] {
        args.push(format!("-getProperty:{prop}"));
    }
    if let Some(c) = &spec.configuration {
        args.push(format!("-p:Configuration={c}"));
    }
    if let Some(f) = framework_override.or(spec.framework.as_deref()) {
        args.push(format!("-p:TargetFramework={f}"));
    }
    args
}

/// The process to spawn for a `dotnet ...` command that must run in `cwd`.
/// The host API has no working-directory option, so a shell does the `cd`.
pub fn dotnet_command(os: Os, cwd: Option<&str>, args: &[String]) -> Result<(String, Vec<String>), String> {
    let Some(cwd) = cwd else {
        return Ok(("dotnet".into(), args.to_vec()));
    };
    if cwd.contains('\0') || args.iter().any(|a| a.contains('\0')) {
        return Err("NUL byte in command".into());
    }
    match os {
        // cwd and every argument travel as positional parameters, never inside the script text,
        // so no value can be interpreted as shell syntax.
        Os::Linux | Os::Mac => {
            let mut argv = vec![
                "-c".to_string(),
                r#"cd -- "$1" && shift && exec dotnet "$@""#.to_string(),
                "sh".to_string(),
                cwd.to_string(),
            ];
            argv.extend(args.iter().cloned());
            Ok(("sh".into(), argv))
        }
        // cmd.exe has no positional-parameter escape hatch, so values are double-quoted and any
        // character that is still special inside quotes (`"`, `%`, `!`) is refused outright.
        Os::Windows => {
            let forbidden = |s: &str| s.chars().any(|c| matches!(c, '"' | '%' | '!' | '\r' | '\n'));
            if forbidden(cwd) || args.iter().any(|a| forbidden(a)) {
                return Err("path or argument contains a character that cannot be passed safely to cmd.exe".into());
            }
            let quoted: Vec<String> = args.iter().map(|a| format!("\"{a}\"")).collect();
            let script = format!("cd /d \"{cwd}\" && dotnet {}", quoted.join(" "));
            Ok(("cmd".into(), vec!["/D".into(), "/S".into(), "/C".into(), format!("\"{script}\"")]))
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct BuildProps {
    pub target_path: String,
    pub output_type: String,
    pub target_frameworks: String,
}

pub fn parse_msbuild_properties(stdout: &str) -> Result<BuildProps, String> {
    let start = stdout.find('{').ok_or("MSBuild printed no JSON (needs .NET SDK 8.0.100 or newer)")?;
    let json: Value = serde_json::from_str(&stdout[start..]).map_err(|e| format!("cannot parse MSBuild output: {e}"))?;
    let props = json.get("Properties").ok_or("MSBuild output has no Properties")?;
    let get = |k: &str| props.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let frameworks = {
        let many = get("TargetFrameworks");
        if many.is_empty() { get("TargetFramework") } else { many }
    };
    Ok(BuildProps { target_path: get("TargetPath"), output_type: get("OutputType"), target_frameworks: frameworks })
}

/// Highest `netX.Y` wins; anything else (netstandard, netcoreapp) ranks by order of appearance.
pub fn pick_framework(list: &str) -> Option<String> {
    let mut best: Option<(u32, u32, String)> = None;
    let mut fallback: Option<String> = None;
    for tfm in list.split(';').map(str::trim).filter(|t| !t.is_empty()) {
        fallback.get_or_insert_with(|| tfm.to_string());
        let version = tfm.strip_prefix("net").and_then(|r| r.split('-').next()).and_then(|v| {
            let (major, minor) = v.split_once('.').unwrap_or((v, "0"));
            Some((major.parse::<u32>().ok()?, minor.parse::<u32>().ok()?))
        });
        if let Some((maj, min)) = version {
            if maj >= 5 && best.as_ref().map_or(true, |(a, b, _)| (maj, min) > (*a, *b)) {
                best = Some((maj, min, tfm.to_string()));
            }
        }
    }
    best.map(|(_, _, t)| t).or(fallback)
}

pub fn check_runnable(props: &BuildProps) -> Result<(), String> {
    if props.target_path.is_empty() {
        return Err("MSBuild reported no output assembly".into());
    }
    match props.output_type.as_str() {
        "Exe" | "WinExe" => Ok(()),
        other => Err(format!("project output type is {other:?}, not an executable, so there is nothing to launch")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_a_typical_dotnet_run() {
        let spec = parse_dotnet_run(&v(&["run", "--project", "src/Api", "-c", "Release", "--no-build", "--", "--port", "5000"])).unwrap();
        assert_eq!(spec.project.as_deref(), Some("src/Api"));
        assert_eq!(spec.configuration.as_deref(), Some("Release"));
        assert_eq!(spec.program_args, v(&["--port", "5000"]));
    }

    #[test]
    fn handles_inline_values_env_and_property_switches() {
        let spec = parse_dotnet_run(&v(&["run", "--framework=net10.0", "-p:Foo=1", "-e", "A=b", "--launch-profile", "https"])).unwrap();
        assert_eq!(spec.framework.as_deref(), Some("net10.0"));
        assert_eq!(spec.project, None);
        assert_eq!(spec.env, vec![("A".to_string(), "b".to_string())]);
    }

    #[test]
    fn rejects_what_it_cannot_debug() {
        assert!(parse_dotnet_run(&v(&["build"])).is_none());
        assert!(parse_dotnet_run(&v(&["run", "app.cs"])).is_none());
        assert!(parse_dotnet_run(&v(&["run", "--project"])).is_none());
        assert!(parse_dotnet_run(&[]).is_none());
    }

    #[test]
    fn build_args_round_trip() {
        let spec = parse_dotnet_run(&v(&["run", "-p", "a.csproj", "-c", "Debug", "-f", "net10.0"])).unwrap();
        let back = parse_build_args(&build_args(&spec));
        assert_eq!((back.project, back.configuration, back.framework), (spec.project, spec.configuration, spec.framework));
    }

    #[test]
    fn program_args_survive_the_env_round_trip() {
        let args = v(&["--name", "a b", "quote\"d", "ünï"]);
        assert_eq!(decode_program_args(&encode_program_args(&args)).unwrap(), args);
        assert!(decode_program_args("not json").is_err());
    }

    #[test]
    fn project_directory_becomes_the_working_directory() {
        assert_eq!(resolve_project(Some("/w"), Some("src/Api")), (Some("/w/src/Api".into()), None));
        assert_eq!(resolve_project(Some("/w"), Some("/abs/Api")), (Some("/abs/Api".into()), None));
        assert_eq!(resolve_project(Some("/w"), Some("a.csproj")), (Some("/w".into()), Some("a.csproj".into())));
        assert_eq!(resolve_project(None, None), (None, None));
        assert_eq!(debuggee_cwd(None, Some("/w/src/a.csproj")).as_deref(), Some("/w/src"));
    }

    #[test]
    fn msbuild_query_carries_configuration_and_framework() {
        let spec = RunSpec { configuration: Some("Release".into()), ..Default::default() };
        let args = msbuild_query_args(Some("a.csproj"), &spec, Some("net10.0"));
        assert!(args.contains(&"-getProperty:TargetPath".to_string()));
        assert!(args.contains(&"-p:Configuration=Release".to_string()));
        assert!(args.contains(&"-p:TargetFramework=net10.0".to_string()));
        assert_eq!(args[1], "a.csproj");
    }

    #[test]
    fn unix_wrapper_never_puts_values_in_the_script() {
        let hostile = "/tmp/x; rm -rf ~ #";
        let (cmd, argv) = dotnet_command(Os::Linux, Some(hostile), &v(&["msbuild", "$(evil).csproj"])).unwrap();
        assert_eq!(cmd, "sh");
        assert!(!argv[1].contains("rm") && !argv[1].contains("evil"));
        assert_eq!(&argv[3..], &[hostile.to_string(), "msbuild".into(), "$(evil).csproj".into()]);
        assert_eq!(dotnet_command(Os::Linux, None, &v(&["msbuild"])).unwrap().0, "dotnet");
    }

    #[test]
    fn windows_wrapper_refuses_cmd_metacharacters() {
        assert!(dotnet_command(Os::Windows, Some("C:\\a\\b"), &v(&["msbuild"])).is_ok());
        for bad in ["C:\\a\"&calc", "C:\\%PATH%", "C:\\a!b"] {
            assert!(dotnet_command(Os::Windows, Some(bad), &v(&["msbuild"])).is_err(), "{bad}");
        }
        assert!(dotnet_command(Os::Windows, Some("C:\\a"), &v(&["x\"y"])).is_err());
    }

    #[test]
    fn parses_msbuild_json_even_with_leading_noise() {
        let out = "MSBuild version 17\n{\n \"Properties\": {\"TargetPath\": \"/w/bin/Debug/net10.0/App.dll\", \"OutputType\": \"Exe\", \"TargetFramework\": \"net10.0\", \"TargetFrameworks\": \"\"}\n}";
        let props = parse_msbuild_properties(out).unwrap();
        assert_eq!(props.target_path, "/w/bin/Debug/net10.0/App.dll");
        assert_eq!(props.target_frameworks, "net10.0");
        assert!(check_runnable(&props).is_ok());
        assert!(parse_msbuild_properties("error MSB1009").is_err());
    }

    #[test]
    fn library_projects_are_not_launchable() {
        let props = BuildProps { target_path: "/x/Lib.dll".into(), output_type: "Library".into(), target_frameworks: String::new() };
        assert!(check_runnable(&props).unwrap_err().contains("not an executable"));
    }

    #[test]
    fn picks_the_newest_modern_framework() {
        assert_eq!(pick_framework("net8.0;net10.0;netstandard2.0").as_deref(), Some("net10.0"));
        assert_eq!(pick_framework("net9.0-windows;net8.0").as_deref(), Some("net9.0-windows"));
        assert_eq!(pick_framework("netstandard2.0").as_deref(), Some("netstandard2.0"));
        assert_eq!(pick_framework(""), None);
    }
}
