use std::io;

use cargo_upwell::{ProviderSelection, ResourceExplanation};
use upwell_tooling_schema::{
    PreviewComponent, PreviewFailure, PreviewOutcome, PreviewRole, TransitionPreview,
};

use super::super::detail::{terminal_text, write_heading};

/// Writes why each provider mapping of the explained resource is or is not selected.
pub(super) fn write_selection(
    explanation: &ResourceExplanation,
    color: bool,
    output: &mut dyn io::Write,
) -> io::Result<()> {
    if explanation.selection.is_empty() {
        return Ok(());
    }

    write_heading(output, "Provider Selection", color)?;

    for selection in &explanation.selection {
        write_provider(selection, output)?;
    }

    Ok(())
}

fn write_provider(selection: &ProviderSelection, output: &mut dyn io::Write) -> io::Result<()> {
    let state = if selection.effective {
        "effective"
    } else {
        "excluded"
    };

    writeln!(
        output,
        "  {} [{state}] role={} condition={}",
        terminal_text(&selection.provider),
        terminal_text(selection.role.as_deref().unwrap_or("unknown")),
        terminal_text(selection.condition_state.as_deref().unwrap_or("unknown"))
    )?;

    for step in &selection.conditions {
        writeln!(
            output,
            "    {:indent$}{} {} -> {}{}",
            "",
            terminal_text(&step.id),
            terminal_text(step.predicate.as_deref().unwrap_or("condition")),
            terminal_text(step.outcome.as_deref().unwrap_or("unknown")),
            step.detail
                .as_deref()
                .map(|detail| format!(" ({})", terminal_text(detail)))
                .unwrap_or_default(),
            indent = step.depth * 2
        )?;
    }

    for resolved in &selection.resolved_by {
        writeln!(
            output,
            "    resolved by {}{}",
            terminal_text(&resolved.consumer),
            resolved
                .reason
                .as_deref()
                .map(|reason| format!(" ({})", terminal_text(reason)))
                .unwrap_or_default()
        )?;
    }

    Ok(())
}

/// Writes the plan of the previewed config change, when the probe produced one.
pub(super) fn write_transition(
    explanation: &ResourceExplanation,
    color: bool,
    output: &mut dyn io::Write,
) -> io::Result<()> {
    let Some(preview) = &explanation.transition else {
        return Ok(());
    };

    write_heading(output, "Transition Preview", color)?;
    writeln!(
        output,
        "  assignments: {}",
        terminal_text(&preview.assignments.join(", "))
    )?;
    writeln!(output, "  outcome: {}", outcome_text(&preview.outcome))?;
    writeln!(output, "  evaluation: {}", preview.evaluation.as_str())?;

    for binding in &preview.changed {
        writeln!(
            output,
            "  changed {} ({})",
            terminal_text(&binding.path),
            terminal_text(&binding.type_name)
        )?;
    }

    for change in &preview.eligibility {
        writeln!(
            output,
            "  eligibility {}: {} -> {}",
            terminal_text(&change.component),
            change.before.as_str(),
            change.after.as_str()
        )?;
    }

    write_components(preview, &explanation.resource.id, output)?;

    if !preview.displaced.is_empty() {
        writeln!(
            output,
            "  displaced: {}",
            terminal_text(&preview.displaced.join(", "))
        )?;
    }

    Ok(())
}

fn write_components(
    preview: &TransitionPreview,
    focus: &str,
    output: &mut dyn io::Write,
) -> io::Result<()> {
    for component in &preview.components {
        let marker = if focus.strip_prefix("component:") == Some(component.component.as_str()) {
            "*"
        } else {
            " "
        };

        writeln!(
            output,
            " {marker}{} {} [{}] strategy={}",
            component.action.as_str(),
            terminal_text(&component.component),
            role_text(component),
            component
                .strategy
                .map_or("none", |strategy| strategy.as_str())
        )?;

        for reason in &component.reasons {
            let path = reason
                .path
                .iter()
                .map(|demand| match &demand.requested {
                    Some(requested) => {
                        format!("{}#{} ({requested})", demand.consumer, demand.ordinal)
                    }
                    None => format!("{}#{}", demand.consumer, demand.ordinal),
                })
                .collect::<Vec<_>>();

            writeln!(
                output,
                "    {}: {}",
                reason.kind.as_str(),
                terminal_text(&path.join(" <- "))
            )?;
        }
    }

    Ok(())
}

fn outcome_text(outcome: &PreviewOutcome) -> String {
    match outcome {
        PreviewOutcome::Planned => String::from("planned"),
        PreviewOutcome::NoOp => String::from("no_op"),
        PreviewOutcome::Rejected { phase, failure } => {
            format!("rejected at {} ({})", phase.as_str(), failure_text(failure))
        }
        _ => String::from("unknown"),
    }
}

fn failure_text(failure: &PreviewFailure) -> String {
    match failure {
        PreviewFailure::ConfigLoad => String::from("config_load"),
        PreviewFailure::ConfigBind { path, type_name } => format!(
            "config_bind {} as {}",
            terminal_text(path),
            terminal_text(type_name)
        ),
        PreviewFailure::Panicked => String::from("panicked"),
        PreviewFailure::ConditionInvalid => String::from("condition_invalid"),
        PreviewFailure::GraphInvalid => String::from("graph_invalid"),
        PreviewFailure::RestartRequired { component, reason } => format!(
            "restart_required {}: {}",
            terminal_text(component),
            terminal_text(reason)
        ),
        _ => String::from("other"),
    }
}

fn role_text(component: &PreviewComponent) -> String {
    match &component.role {
        PreviewRole::Singleton => String::from("singleton"),
        PreviewRole::FutureScope { scope } => format!("future_scope {}", terminal_text(scope)),
        PreviewRole::Transient => String::from("transient"),
        _ => String::from("unknown"),
    }
}
