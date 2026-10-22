mod cli;
mod output;
mod render;
mod render_runtime;
mod version;

use std::io::{self, IsTerminal as _};
use std::path::Path;
use std::process::ExitCode;

use cargo_upwell::{
    BuiltInRenderer, CancellationToken, Catalog, CommandExitCode, GraphQuery, GraphQueryError,
    InitError, ProbeOptions, ProbeRequestError, RendererCommand, ResourceExplanation,
    TemplateSelection, TemplateSource, ToolingProbe, completion, init_project,
    probe_request_exit_code, run_command_with_options, run_probe_with_options,
};
use upwell_tooling_schema::{Diagnostic, ProbeEnvelope, ProbeOutcome, ToolingDocument};

use crate::cli::{Cli, CommandRequest, ExplainFormat, GraphFormat, InspectFilters, TerminalPolicy};
use crate::output::{terminal_output_enabled, write_export, write_text};
use crate::render_runtime::{registry as renderer_registry, render_selected};

fn main() -> ExitCode {
    clap_complete::CompleteEnv::with_factory(cli::completion_command)
        .bin("cargo-upwell")
        .completer("cargo-upwell")
        .complete();

    execute(Cli::parse_cargo().into_request())
}

fn execute(request: CommandRequest) -> ExitCode {
    let cancellation = CancellationToken::default();
    let interactive = io::stderr().is_terminal();

    match request {
        CommandRequest::GenerateCompletions(shell) => generate_completions(shell),
        CommandRequest::RefreshCompletions(discovery) => {
            let options = probe_options(interactive);

            match run_probe_with_options(&discovery, &cancellation, options) {
                Ok(probe) => match refresh_completion_cache(&probe, &discovery) {
                    Ok(()) => {
                        println!(
                            "Refreshed completions for {} ({})",
                            probe.target.package_name, probe.target.binary_name
                        );
                        ExitCode::SUCCESS
                    }
                    Err(error) => {
                        eprintln!("cargo upwell could not cache completion candidates: {error}");
                        operational_failure()
                    }
                },
                Err(error) => probe_error(error),
            }
        }
        CommandRequest::Init(mut request) => {
            if let TemplateSelection::Catalog {
                template: None,
                catalog_path,
            } = &request.template
            {
                match select_template(catalog_path.as_deref()) {
                    Ok(template) => {
                        request.template = TemplateSelection::Catalog {
                            template: Some(template),
                            catalog_path: catalog_path.clone(),
                        };
                    }
                    Err(error) => return init_error(error),
                }
            }

            match init_project(request) {
                Ok(result) => {
                    println!("Created {} from {}", result.path.display(), result.template);

                    ExitCode::SUCCESS
                }
                Err(error) => init_error(error),
            }
        }
        CommandRequest::Templates { catalog_path } => {
            match Catalog::load(catalog_path.as_deref()) {
                Ok(catalog) => finish_output(
                    write_templates(&catalog, &mut io::stdout().lock()),
                    ExitCode::SUCCESS,
                    "template catalog",
                ),
                Err(error) => init_error(InitError::Catalog(error)),
            }
        }
        CommandRequest::Report {
            command,
            discovery,
            format,
        } => {
            let options = probe_options(interactive);
            let report = run_command_with_options(command, &discovery, &cancellation, options);
            let exit_code = report.exit_code().code();
            let command = match command {
                cargo_upwell::CommandKind::Check => RendererCommand::Check,
                cargo_upwell::CommandKind::Doctor => RendererCommand::Doctor,
            };
            let registry = renderer_registry();
            let Some(selected) = registry.resolve(command, &format) else {
                return unknown_renderer(command, &format);
            };
            let fallback = Some(registry.command_fallback(command));
            let payload = report
                .to_json()
                .map(String::into_bytes)
                .map_err(io::Error::other);
            let result = render_selected(
                selected,
                fallback,
                &report.schema,
                &[],
                false,
                terminal_output_enabled(),
                payload,
                |native, output| match native {
                    BuiltInRenderer::ReportTerminal => {
                        render::write_report(&report, crate::cli::ReportFormat::Terminal, output)
                    }
                    BuiltInRenderer::ReportJson => {
                        render::write_report(&report, crate::cli::ReportFormat::Json, output)
                    }
                    _ => unreachable!("report registry selected a report renderer"),
                },
                &mut io::stdout().lock(),
            );

            finish_output(result, ExitCode::from(exit_code), "command report")
        }
        CommandRequest::Inspect {
            discovery,
            format,
            filters,
            color,
            pager,
        } => {
            let registry = renderer_registry();
            let Some(selected) = registry.resolve(RendererCommand::Inspect, &format) else {
                return unknown_renderer(RendererCommand::Inspect, &format);
            };
            if let Err(exit) = validate_terminal_options(selected, color, pager) {
                return exit;
            }
            if format == "json" && !filters.is_empty() {
                eprintln!(
                    "cargo upwell inspect filters require text output; canonical JSON is always the complete document"
                );

                return ExitCode::from(CommandExitCode::Misuse.code());
            }

            let options = probe_options(interactive);

            match run_probe_with_options(&discovery, &cancellation, options) {
                Ok(probe) => {
                    let _ = refresh_completion_cache(&probe, &discovery);
                    inspect(probe, &format, filters, color, pager)
                }
                Err(error) => probe_error(error),
            }
        }
        CommandRequest::Export {
            discovery,
            format,
            output,
        } => {
            let registry = renderer_registry();
            let Some(selected) = registry.resolve(RendererCommand::Export, &format) else {
                return unknown_renderer(RendererCommand::Export, &format);
            };
            if output.is_some() && !selected.format().capabilities().output_file {
                eprintln!("cargo upwell export format `{format}` does not support `--output`");
                return ExitCode::from(CommandExitCode::Misuse.code());
            }
            if matches!(selected.descriptor().implementation(), cargo_upwell::RendererImplementation::Component(component) if !component.utf8())
                && output.is_none()
            {
                eprintln!("cargo upwell binary export format `{format}` requires `--output`");
                return ExitCode::from(CommandExitCode::Misuse.code());
            }
            match run_probe_with_options(&discovery, &cancellation, probe_options(interactive)) {
                Ok(probe) => {
                    let _ = refresh_completion_cache(&probe, &discovery);
                    export(probe.probe.envelope, &format, output.as_deref())
                }
                Err(error) => probe_error(error),
            }
        }
        CommandRequest::Graph {
            discovery,
            format,
            query,
            color,
            pager,
        } => {
            let registry = renderer_registry();
            let Some(selected) = registry.resolve(RendererCommand::Graph, &format) else {
                return unknown_renderer(RendererCommand::Graph, &format);
            };
            if let Err(exit) = validate_terminal_options(selected, color, pager) {
                return exit;
            }
            match run_probe_with_options(&discovery, &cancellation, probe_options(interactive)) {
                Ok(probe) => {
                    let _ = refresh_completion_cache(&probe, &discovery);
                    graph(probe, &format, query, color, pager)
                }
                Err(error) => probe_error(error),
            }
        }
        CommandRequest::Explain {
            discovery,
            format,
            resource,
            preview,
            color,
            pager,
        } => {
            let registry = renderer_registry();
            let Some(selected) = registry.resolve(RendererCommand::Explain, &format) else {
                return unknown_renderer(RendererCommand::Explain, &format);
            };
            if let Err(exit) = validate_terminal_options(selected, color, pager) {
                return exit;
            }
            let options = ProbeOptions {
                transition_preview: preview,
                ..probe_options(interactive)
            };

            match run_probe_with_options(&discovery, &cancellation, options) {
                Ok(probe) => {
                    let _ = refresh_completion_cache(&probe, &discovery);
                    explain(probe, &format, &resource, color, pager)
                }
                Err(error) => probe_error(error),
            }
        }
    }
}

fn probe_options(interactive: bool) -> ProbeOptions {
    ProbeOptions {
        show_cargo_output: interactive,
        ..ProbeOptions::default()
    }
}

fn generate_completions(shell: clap_complete::Shell) -> ExitCode {
    if shell == clap_complete::Shell::Fish {
        return finish_output(
            write_fish_completions(&mut io::stdout().lock()),
            ExitCode::SUCCESS,
            "completion registration",
        );
    }
    if shell == clap_complete::Shell::PowerShell {
        return finish_output(
            write_powershell_completions(&mut io::stdout().lock()),
            ExitCode::SUCCESS,
            "completion registration",
        );
    }

    let completer: &dyn clap_complete::env::EnvCompleter = match shell {
        clap_complete::Shell::Bash => &clap_complete::env::Bash,
        clap_complete::Shell::Elvish => &clap_complete::env::Elvish,
        clap_complete::Shell::Fish => &clap_complete::env::Fish,
        clap_complete::Shell::PowerShell => &clap_complete::env::Powershell,
        clap_complete::Shell::Zsh => &clap_complete::env::Zsh,
        _ => {
            eprintln!("cargo upwell does not support completion generation for {shell}");
            return ExitCode::from(CommandExitCode::Misuse.code());
        }
    };
    let result = completer.write_registration(
        "COMPLETE",
        "cargo-upwell",
        "cargo-upwell",
        "cargo-upwell",
        &mut io::stdout().lock(),
    );

    finish_output(result, ExitCode::SUCCESS, "completion registration")
}

fn write_powershell_completions(output: &mut dyn io::Write) -> io::Result<()> {
    writeln!(
        output,
        r#"Register-ArgumentCompleter -Native -CommandName cargo-upwell -ScriptBlock {{
    param($wordToComplete, $commandAst, $cursorPosition)

    $previousComplete = $env:COMPLETE
    $previousIndex = $env:_CLAP_COMPLETE_INDEX
    $env:COMPLETE = "powershell"
    try {{
        $elements = @($commandAst.CommandElements | Where-Object {{
            $_.Extent.StartOffset -lt $cursorPosition
        }})
        $tokens = @($elements | ForEach-Object {{
            if ($_ -is [System.Management.Automation.Language.StringConstantExpressionAst]) {{
                $_.Value
            }} else {{
                $_.Extent.Text
            }}
        }})
        $current = $elements | Select-Object -Last 1
        if ($null -eq $current -or $current.Extent.EndOffset -lt $cursorPosition) {{
            $tokens += $wordToComplete
        }} elseif ($tokens.Count -gt 0) {{
            $tokens[$tokens.Count - 1] = $wordToComplete
        }}
        $env:_CLAP_COMPLETE_INDEX = [string]($tokens.Count - 1)
        & cargo-upwell -- @tokens | ForEach-Object {{
            $parts = $_ -split "`t", 2
            $help = if ($parts.Count -eq 2) {{ $parts[1] }} else {{ $parts[0] }}
            [System.Management.Automation.CompletionResult]::new(
                $parts[0], $parts[0], 'ParameterValue', $help
            )
        }}
    }} finally {{
        if ($null -eq $previousComplete) {{ Remove-Item Env:\COMPLETE -ErrorAction SilentlyContinue }}
        else {{ $env:COMPLETE = $previousComplete }}
        if ($null -eq $previousIndex) {{ Remove-Item Env:\_CLAP_COMPLETE_INDEX -ErrorAction SilentlyContinue }}
        else {{ $env:_CLAP_COMPLETE_INDEX = $previousIndex }}
    }}
}}"#
    )
}

fn write_fish_completions(output: &mut dyn io::Write) -> io::Result<()> {
    use clap_complete::env::EnvCompleter as _;

    clap_complete::env::Fish.write_registration(
        "COMPLETE",
        "cargo-upwell",
        "cargo-upwell",
        "cargo-upwell",
        output,
    )?;
    writeln!(
        output,
        r#"
function __cargo_upwell_complete
    set --local tokens (commandline --current-process --tokenize --cut-at-cursor)
    if test (count $tokens) -ge 2
        set --erase tokens[1..2]
    end
    COMPLETE=fish cargo-upwell -- cargo-upwell $tokens (commandline --current-token)
end

function __cargo_upwell_using_subcommand
    set --local tokens (commandline --current-process --tokenize --cut-at-cursor)
    test (count $tokens) -ge 2; and test "$tokens[1]" = cargo; and test "$tokens[2]" = upwell
end

complete --keep-order --exclusive --command cargo \
    --condition '__cargo_upwell_using_subcommand' \
    --arguments '(__cargo_upwell_complete)'
"#
    )
}

fn refresh_completion_cache(
    probe: &ToolingProbe,
    discovery: &cargo_upwell::DiscoveryRequest,
) -> Result<(), completion::CacheError> {
    completion::refresh(probe, &discovery.features, discovery.target.as_deref())
}

fn init_error(error: InitError) -> ExitCode {
    eprintln!("cargo upwell could not initialize the project: {error}");

    match error {
        InitError::MissingName
        | InitError::ReservedValue(_)
        | InitError::InvalidTemplatePath(_)
        | InitError::DestinationExists(_)
        | InitError::MissingTemplate => ExitCode::from(CommandExitCode::Misuse.code()),
        InitError::Catalog(cargo_upwell::CatalogError::Read { .. }) => operational_failure(),
        InitError::Catalog(_) => ExitCode::from(CommandExitCode::Misuse.code()),
        _ => operational_failure(),
    }
}

fn select_template(path: Option<&Path>) -> Result<String, InitError> {
    let catalog = Catalog::load(path)?;
    let templates = catalog.templates().collect::<Vec<_>>();

    if !io::stderr().is_terminal() {
        eprintln!("Template selection requires an interactive terminal.");
        write_templates(&catalog, &mut io::stderr().lock()).map_err(|source| {
            InitError::Generate {
                template: String::from("catalog"),
                source: anyhow::Error::new(source),
            }
        })?;
        eprintln!("Pass --template <ID>, or use --template-path <PATH>.");

        return Err(InitError::MissingTemplate);
    }

    let labels = templates
        .iter()
        .map(|template| format!("{}  {}", template.id(), template.description()))
        .collect::<Vec<_>>();
    let selection = dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
        .with_prompt("Choose an Upwell project template")
        .items(&labels)
        .default(0)
        .max_length(12)
        .interact_opt()
        .map_err(|source| InitError::Generate {
            template: String::from("selector"),
            source: anyhow::Error::new(source),
        })?
        .ok_or(InitError::MissingTemplate)?;

    Ok(templates[selection].id().to_owned())
}

fn write_templates(catalog: &Catalog, output: &mut dyn io::Write) -> io::Result<()> {
    for template in catalog.templates() {
        let source = match template.source() {
            TemplateSource::Local(path) => format!("path {}", path.display()),
            TemplateSource::Git {
                repository,
                reference,
            } => reference.as_ref().map_or_else(
                || format!("git {repository}"),
                |reference| format!("git {repository} ({reference})"),
            ),
        };

        writeln!(
            output,
            "{:<24} {:<54} {}",
            template.id(),
            template.description(),
            source
        )?;
    }

    Ok(())
}

fn inspect(
    probe: ToolingProbe,
    format: &str,
    filters: InspectFilters,
    color: TerminalPolicy,
    pager: TerminalPolicy,
) -> ExitCode {
    let envelope = probe.probe.envelope;
    let document = match envelope.outcome {
        ProbeOutcome::Success { document } => document,
        ProbeOutcome::Failure { failure } => {
            return preparation_failure("inspect", &failure.diagnostics);
        }
    };
    let exit_code = document_exit_code(&document);
    let registry = renderer_registry();
    let Some(selected) = registry.resolve(RendererCommand::Inspect, format) else {
        return unknown_renderer(RendererCommand::Inspect, format);
    };
    if let Err(exit) = validate_terminal_options(selected, color, pager) {
        return exit;
    }
    let fallback = Some(registry.command_fallback(RendererCommand::Inspect));
    let resources = render::selected_resources(&document, &filters)
        .into_iter()
        .map(|resource| resource.id.clone())
        .collect::<Vec<_>>();
    let payload = render::project_inspection(&document, &filters)
        .and_then(|document| document.to_canonical_json().map_err(io::Error::other))
        .map(String::into_bytes);
    let write = |color, output: &mut dyn io::Write| {
        render_selected(
            selected,
            fallback,
            &document.schema,
            &resources,
            color,
            terminal_output_enabled(),
            payload,
            |native, output| match native {
                BuiltInRenderer::InspectText => {
                    render::write_inspection(&document, &filters, color, output)
                }
                BuiltInRenderer::InspectJson => write_document_json(&document, output),
                _ => unreachable!("inspect registry selected an inspect renderer"),
            },
            output,
        )
    };
    let pager = if selected.format().capabilities().pager {
        pager
    } else {
        TerminalPolicy::Never
    };
    let result = write_text(color, pager, write);

    finish_output(result, exit_code, "inspection")
}

fn graph(
    probe: ToolingProbe,
    format: &str,
    query: GraphQuery,
    color: TerminalPolicy,
    pager: TerminalPolicy,
) -> ExitCode {
    let envelope = probe.probe.envelope;
    let (view, exit_code) = match envelope.outcome {
        ProbeOutcome::Success { document } => {
            let view = match query.execute(&document) {
                Ok(view) => view,
                Err(error) => return graph_query_error("graph", error),
            };
            let exit_code = document_exit_code(&document);

            (view, exit_code)
        }
        ProbeOutcome::Failure { failure } => {
            let view = match query.execute_failure(envelope.schema, &envelope.identity, &failure) {
                Ok(view) => view,
                Err(error) => return graph_query_error("graph", error),
            };

            (view, validation_failure())
        }
    };
    let registry = renderer_registry();
    let Some(selected) = registry.resolve(RendererCommand::Graph, format) else {
        return unknown_renderer(RendererCommand::Graph, format);
    };
    if let Err(exit) = validate_terminal_options(selected, color, pager) {
        return exit;
    }
    let fallback = Some(registry.command_fallback(RendererCommand::Graph));
    let resources = view
        .nodes
        .iter()
        .map(|resource| resource.id.clone())
        .collect::<Vec<_>>();
    let payload = view
        .to_canonical_json()
        .map(String::into_bytes)
        .map_err(io::Error::other);
    let write = |color, output: &mut dyn io::Write| {
        render_selected(
            selected,
            fallback,
            &view.schema,
            &resources,
            color,
            terminal_output_enabled(),
            payload,
            |native, output| {
                let format = graph_format(native);
                render::write_graph(&view, format, color, output)
            },
            output,
        )
    };
    let pager = if selected.format().capabilities().pager {
        pager
    } else {
        TerminalPolicy::Never
    };
    let result = write_text(color, pager, write);

    finish_output(result, exit_code, "graph")
}

fn explain(
    probe: ToolingProbe,
    format: &str,
    resource: &str,
    color: TerminalPolicy,
    pager: TerminalPolicy,
) -> ExitCode {
    let envelope = probe.probe.envelope;
    let document = match envelope.outcome {
        ProbeOutcome::Success { document } => document,
        ProbeOutcome::Failure { failure } => {
            return preparation_failure("explain", &failure.diagnostics);
        }
    };
    let explanation = match ResourceExplanation::query(&document, resource) {
        Ok(explanation) => explanation,
        Err(error) => return graph_query_error("explain", error),
    };
    let exit_code = document_exit_code(&document);
    let registry = renderer_registry();
    let Some(selected) = registry.resolve(RendererCommand::Explain, format) else {
        return unknown_renderer(RendererCommand::Explain, format);
    };
    if let Err(exit) = validate_terminal_options(selected, color, pager) {
        return exit;
    }
    let fallback = Some(registry.command_fallback(RendererCommand::Explain));
    let resources = vec![explanation.resource.id.clone()];
    let payload = explanation
        .to_canonical_json()
        .map(String::into_bytes)
        .map_err(io::Error::other);
    let write = |color, output: &mut dyn io::Write| {
        render_selected(
            selected,
            fallback,
            &explanation.schema,
            &resources,
            color,
            terminal_output_enabled(),
            payload,
            |native, output| {
                let format = match native {
                    BuiltInRenderer::ExplainText => ExplainFormat::Text,
                    BuiltInRenderer::ExplainJson => ExplainFormat::Json,
                    _ => unreachable!("explain registry selected an explain renderer"),
                };
                render::write_explanation(&explanation, format, color, output)
            },
            output,
        )
    };
    let pager = if selected.format().capabilities().pager {
        pager
    } else {
        TerminalPolicy::Never
    };
    let result = write_text(color, pager, write);

    finish_output(result, exit_code, "explanation")
}

fn export(envelope: ProbeEnvelope, format: &str, output: Option<&Path>) -> ExitCode {
    let exit_code = if envelope.is_success() {
        envelope_document(&envelope)
            .map(document_exit_code)
            .unwrap_or_else(validation_failure)
    } else {
        validation_failure()
    };
    if format == "document" && !envelope.is_success() {
        eprintln!("cargo upwell export cannot emit a document from a failed probe");

        return validation_failure();
    }
    let registry = renderer_registry();
    let Some(selected) = registry.resolve(RendererCommand::Export, format) else {
        return unknown_renderer(RendererCommand::Export, format);
    };
    if output.is_some() && !selected.format().capabilities().output_file {
        eprintln!("cargo upwell export format `{format}` does not support `--output`");
        return ExitCode::from(CommandExitCode::Misuse.code());
    }
    let fallback = Some(if envelope.is_success() {
        registry.command_fallback(RendererCommand::Export)
    } else {
        registry
            .native_fallback(RendererCommand::Export, "envelope")
            .expect("failed probes have a native envelope fallback")
    });
    let schema = envelope.schema.clone();
    let resources = envelope_document(&envelope)
        .map(|document| {
            document
                .resources
                .iter()
                .map(|resource| resource.id.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let payload = envelope
        .to_json()
        .map(String::into_bytes)
        .map_err(io::Error::other);
    let result = write_export(output, |writer| {
        render_selected(
            selected,
            fallback,
            &schema,
            &resources,
            false,
            false,
            payload,
            |native, output| match native {
                BuiltInRenderer::ExportDocument => match envelope_document(&envelope) {
                    Some(document) => write_document_json(document, output),
                    None => Err(io::Error::other(
                        "a failed probe has no canonical tooling document",
                    )),
                },
                BuiltInRenderer::ExportEnvelope => {
                    let json = envelope.to_json().map_err(io::Error::other)?;
                    writeln!(output, "{json}")
                }
                _ => unreachable!("export registry selected an export renderer"),
            },
            writer,
        )
    });

    finish_output(result, exit_code, "export")
}

fn graph_format(renderer: BuiltInRenderer) -> GraphFormat {
    match renderer {
        BuiltInRenderer::GraphText => GraphFormat::Text,
        BuiltInRenderer::GraphMermaid => GraphFormat::Mermaid,
        BuiltInRenderer::GraphDot => GraphFormat::Dot,
        BuiltInRenderer::GraphJson => GraphFormat::Json,
        _ => unreachable!("graph registry selected a graph renderer"),
    }
}

fn unknown_renderer(command: RendererCommand, format: &str) -> ExitCode {
    eprintln!("cargo upwell could not resolve {command} renderer format `{format}`");

    ExitCode::from(CommandExitCode::Misuse.code())
}

fn validate_terminal_options(
    selected: cargo_upwell::ResolvedRenderer<'_>,
    color: TerminalPolicy,
    pager: TerminalPolicy,
) -> Result<(), ExitCode> {
    for (option, policy, supported) in [
        ("--color", color, selected.format().capabilities().color),
        ("--pager", pager, selected.format().capabilities().pager),
    ] {
        if policy == TerminalPolicy::Always && !supported {
            eprintln!(
                "cargo upwell format `{}` does not support `{option} always`",
                selected.format().id()
            );
            return Err(ExitCode::from(CommandExitCode::Misuse.code()));
        }
    }

    Ok(())
}

fn finish_output(result: io::Result<()>, exit_code: ExitCode, output: &str) -> ExitCode {
    match result {
        Ok(()) => exit_code,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => exit_code,
        Err(error) => {
            eprintln!("cargo upwell could not write its {output}: {error}");

            operational_failure()
        }
    }
}

fn write_document_json(document: &ToolingDocument, output: &mut dyn io::Write) -> io::Result<()> {
    let json = document.to_canonical_json().map_err(io::Error::other)?;

    writeln!(output, "{json}")
}

fn envelope_document(envelope: &ProbeEnvelope) -> Option<&ToolingDocument> {
    match &envelope.outcome {
        ProbeOutcome::Success { document } => Some(document),
        ProbeOutcome::Failure { .. } => None,
    }
}

fn document_exit_code(document: &ToolingDocument) -> ExitCode {
    if document.validation.valid {
        ExitCode::SUCCESS
    } else {
        validation_failure()
    }
}

#[cfg(test)]
mod tests;

fn probe_error(error: ProbeRequestError) -> ExitCode {
    let exit_code = probe_request_exit_code(&error);

    eprintln!("cargo upwell could not read the application tooling probe: {error}");

    ExitCode::from(exit_code.code())
}

fn preparation_failure(command: &str, diagnostics: &[Diagnostic]) -> ExitCode {
    let result = render::write_diagnostics(diagnostics, "", &mut io::stderr().lock());

    if let Err(error) = result
        && error.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!("cargo upwell could not write {command} failure diagnostics: {error}");

        return operational_failure();
    }

    validation_failure()
}

fn graph_query_error(command: &str, error: GraphQueryError) -> ExitCode {
    let command = render::terminal_text(command);
    let error_text = render::terminal_text(&error.to_string());

    eprintln!("cargo upwell {command}: {error_text}");

    if let GraphQueryError::Ambiguous { candidates, .. } = error {
        eprintln!("candidates:");

        for candidate in candidates {
            eprintln!("  {}", render::terminal_text(&candidate));
        }
    }

    ExitCode::from(CommandExitCode::Misuse.code())
}

fn validation_failure() -> ExitCode {
    ExitCode::from(CommandExitCode::ValidationFailure.code())
}

fn operational_failure() -> ExitCode {
    ExitCode::from(CommandExitCode::OperationalFailure.code())
}
