use std::env;
use std::fs;
use std::path::Path;

use dotnet_debugger_core::locator::{self, ENV_RUN_ARGS};
use dotnet_debugger_core::pinned::{self, Arch, Os};
use dotnet_debugger_core::{install, session};
use zed_extension_api::{self as zed, serde_json, Result};

const ADAPTER: &str = "netcoredbg";
const LOCATOR: &str = "dotnet";

#[derive(Default)]
struct DotnetDebugger {
    /// Resolved adapter path, kept for the lifetime of the extension instance so repeat
    /// sessions skip every filesystem probe.
    cached_binary: Option<String>,
}

fn host_os(os: zed::Os) -> Os {
    match os {
        zed::Os::Linux => Os::Linux,
        zed::Os::Mac => Os::Mac,
        zed::Os::Windows => Os::Windows,
    }
}

fn host_arch(arch: zed::Architecture) -> Arch {
    match arch {
        zed::Architecture::Aarch64 => Arch::Aarch64,
        zed::Architecture::X86 => Arch::X86,
        zed::Architecture::X8664 => Arch::X86_64,
    }
}

fn require_adapter(name: &str) -> Result<()> {
    if name == ADAPTER {
        Ok(())
    } else {
        Err(format!("this extension only provides the `{ADAPTER}` adapter, not `{name}`"))
    }
}

fn tail(text: &str) -> String {
    let text = text.trim();
    let start = text.len().saturating_sub(600);
    let start = (start..text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(text.len());
    text[start..].to_string()
}

impl DotnetDebugger {
    fn resolve_binary(&mut self, user_path: Option<String>, worktree: &zed::Worktree) -> Result<String> {
        if let Some(path) = user_path.filter(|p| !p.trim().is_empty()) {
            return Ok(path);
        }
        if let Some(path) = worktree.which("netcoredbg") {
            return Ok(path);
        }
        if let Some(path) = &self.cached_binary {
            if Path::new(path).is_file() {
                return Ok(path.clone());
            }
        }
        let path = install_pinned()?;
        self.cached_binary = Some(path.clone());
        Ok(path)
    }
}

fn install_pinned() -> Result<String> {
    let (os, arch) = zed::current_platform();
    let asset = pinned::asset_for(host_os(os), host_arch(arch))?;
    let root = env::current_dir().map_err(|e| format!("cannot resolve extension directory: {e}"))?;

    if let Some(exe) = install::find_installed(&root, asset) {
        return Ok(exe.to_string_lossy().into_owned());
    }

    let archive_path = format!(".download-{}", asset.name);
    zed::download_file(&pinned::download_url(asset), &archive_path, zed::DownloadedFileType::Uncompressed)
        .map_err(|e| format!("failed to download netcoredbg {}: {e}", pinned::NETCOREDBG_VERSION))?;
    let bytes = fs::read(&archive_path);
    let _ = fs::remove_file(&archive_path);
    let bytes = bytes.map_err(|e| format!("cannot read downloaded archive: {e}"))?;

    let exe = install::install(&root, asset, &bytes, &|path| {
        let relative = path.strip_prefix(&root).unwrap_or(path);
        zed::make_file_executable(&relative.to_string_lossy())
    })?;
    install::prune_stale(&root);
    Ok(exe.to_string_lossy().into_owned())
}

fn query_msbuild(
    os: Os,
    cwd: Option<&str>,
    project: Option<&str>,
    spec: &locator::RunSpec,
    framework: Option<&str>,
) -> Result<locator::BuildProps> {
    let args = locator::msbuild_query_args(project, spec, framework);
    let (command, argv) = locator::dotnet_command(os, cwd, &args)?;
    let output = zed::process::Command::new(command)
        .args(argv)
        .output()
        .map_err(|e| format!("cannot run `dotnet msbuild`: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if output.status != Some(0) {
        return Err(format!(
            "`dotnet msbuild` failed: {} {}",
            tail(&stdout),
            tail(&String::from_utf8_lossy(&output.stderr))
        ));
    }
    locator::parse_msbuild_properties(&stdout)
}

fn build_scenario_task(label: String, command: String, args: Vec<String>, env: zed::EnvVars, cwd: Option<String>) -> zed::BuildTaskDefinition {
    zed::BuildTaskDefinition::Template(zed::BuildTaskDefinitionTemplatePayload {
        locator_name: Some(LOCATOR.to_string()),
        template: zed::TaskTemplate { label, command, args, env, cwd },
    })
}

impl zed::Extension for DotnetDebugger {
    fn new() -> Self {
        Self::default()
    }

    fn get_dap_binary(
        &mut self,
        adapter_name: String,
        config: zed::DebugTaskDefinition,
        user_provided_debug_adapter_path: Option<String>,
        worktree: &zed::Worktree,
    ) -> Result<zed::DebugAdapterBinary> {
        require_adapter(&adapter_name)?;
        let raw: serde_json::Value = serde_json::from_str(&config.config)
            .map_err(|e| format!("debug configuration is not valid JSON: {e}"))?;
        let root = worktree.root_path();
        let (kind, normalized) = session::normalize(raw, &root)?;

        let shell_env = worktree.shell_env();
        let has_dotnet_root = shell_env.iter().any(|(k, _)| k.starts_with("DOTNET_ROOT"));
        if worktree.which("dotnet").is_none() && !has_dotnet_root {
            return Err("the .NET SDK/runtime was not found: put `dotnet` on PATH or set DOTNET_ROOT".into());
        }

        let command = self.resolve_binary(user_provided_debug_adapter_path, worktree)?;
        // netcoredbg locates the CLR through PATH / DOTNET_ROOT; nothing else is forwarded.
        let envs = shell_env
            .into_iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("PATH") || k.starts_with("DOTNET_ROOT"))
            .collect();

        Ok(zed::DebugAdapterBinary {
            command: Some(command),
            arguments: vec!["--interpreter=vscode".to_string()],
            envs,
            cwd: Some(root),
            connection: None,
            request_args: zed::StartDebuggingRequestArguments {
                configuration: normalized.to_string(),
                request: match kind {
                    session::Kind::Launch => zed::StartDebuggingRequestArgumentsRequest::Launch,
                    session::Kind::Attach => zed::StartDebuggingRequestArgumentsRequest::Attach,
                },
            },
        })
    }

    fn dap_request_kind(
        &mut self,
        adapter_name: String,
        config: serde_json::Value,
    ) -> Result<zed::StartDebuggingRequestArgumentsRequest> {
        require_adapter(&adapter_name)?;
        Ok(match session::request_kind(&config)? {
            session::Kind::Launch => zed::StartDebuggingRequestArgumentsRequest::Launch,
            session::Kind::Attach => zed::StartDebuggingRequestArgumentsRequest::Attach,
        })
    }

    fn dap_config_to_scenario(&mut self, config: zed::DebugConfig) -> Result<zed::DebugScenario> {
        require_adapter(&config.adapter)?;
        let (build, json) = match config.request {
            zed::DebugRequest::Launch(launch) => {
                let json = session::launch_config(&session::LaunchParams {
                    program: launch.program.clone(),
                    cwd: launch.cwd.clone(),
                    args: launch.args.clone(),
                    env: launch.envs.clone(),
                    stop_on_entry: config.stop_on_entry,
                });
                let build = session::is_project_file(&launch.program).then(|| {
                    let mut env = launch.envs.clone();
                    env.push((ENV_RUN_ARGS.to_string(), locator::encode_program_args(&launch.args)));
                    build_scenario_task(
                        "dotnet build".into(),
                        "dotnet".into(),
                        vec!["build".into(), launch.program.clone()],
                        env,
                        launch.cwd.clone(),
                    )
                });
                (build, json)
            }
            zed::DebugRequest::Attach(attach) => (None, session::attach_config(attach.process_id)?),
        };
        Ok(zed::DebugScenario {
            label: config.label,
            adapter: config.adapter,
            build,
            config: json.to_string(),
            tcp_connection: None,
        })
    }

    fn dap_locator_create_scenario(
        &mut self,
        locator_name: String,
        build_task: zed::TaskTemplate,
        resolved_label: String,
        debug_adapter_name: String,
    ) -> Option<zed::DebugScenario> {
        if locator_name != LOCATOR || debug_adapter_name != ADAPTER {
            return None;
        }
        let stem = Path::new(&build_task.command).file_stem()?.to_str()?;
        if !stem.eq_ignore_ascii_case("dotnet") {
            return None;
        }
        let spec = locator::parse_dotnet_run(&build_task.args)?;

        let mut env = build_task.env.clone();
        env.extend(spec.env.iter().cloned());
        env.push((ENV_RUN_ARGS.to_string(), locator::encode_program_args(&spec.program_args)));

        Some(zed::DebugScenario {
            label: format!("Debug {resolved_label}"),
            adapter: debug_adapter_name,
            build: Some(build_scenario_task(
                format!("Build {resolved_label}"),
                build_task.command.clone(),
                locator::build_args(&spec),
                env,
                build_task.cwd.clone(),
            )),
            config: r#"{"request":"launch"}"#.to_string(),
            tcp_connection: None,
        })
    }

    fn run_dap_locator(&mut self, locator_name: String, build_task: zed::TaskTemplate) -> Result<zed::DebugRequest> {
        if locator_name != LOCATOR {
            return Err(format!("unknown locator `{locator_name}`"));
        }
        let (os, _) = zed::current_platform();
        let os = host_os(os);

        let spec = locator::parse_build_args(&build_task.args);
        let (query_cwd, project_file) = locator::resolve_project(build_task.cwd.as_deref(), spec.project.as_deref());

        let mut props = query_msbuild(os, query_cwd.as_deref(), project_file.as_deref(), &spec, None)?;
        if spec.framework.is_none() && props.target_frameworks.contains(';') {
            let chosen = locator::pick_framework(&props.target_frameworks)
                .ok_or("project declares no usable target framework")?;
            props = query_msbuild(os, query_cwd.as_deref(), project_file.as_deref(), &spec, Some(&chosen))?;
        }
        locator::check_runnable(&props)?;

        let args = match build_task.env.iter().find(|(k, _)| k == ENV_RUN_ARGS) {
            Some((_, raw)) => locator::decode_program_args(raw)?,
            None => Vec::new(),
        };
        let mut envs: zed::EnvVars = build_task.env.iter().filter(|(k, _)| k != ENV_RUN_ARGS).cloned().collect();
        // `dotnet run` with a launch profile would set these; without them ASP.NET Core
        // silently starts in Production, which is almost never what a debug session wants.
        for (name, value) in [("ASPNETCORE_ENVIRONMENT", "Development"), ("DOTNET_ENVIRONMENT", "Development")] {
            if !envs.iter().any(|(k, _)| k.eq_ignore_ascii_case(name)) {
                envs.push((name.to_string(), value.to_string()));
            }
        }

        Ok(zed::DebugRequest::Launch(zed::LaunchRequest {
            program: props.target_path,
            cwd: locator::debuggee_cwd(build_task.cwd.as_deref(), project_file.as_deref()),
            args,
            envs,
        }))
    }
}

zed::register_extension!(DotnetDebugger);
