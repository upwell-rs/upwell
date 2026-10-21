//! The transition audit: hooks that inspect each proposed change and observe how it ended.

use std::sync::Mutex;

use upwell::{
    NodeAction, PendingTransition, RuntimeTransitionReport, TransitionAborted, TransitionCommitted,
    TransitionOutcome, TransitionProposal, TransitionVerdict, component, methods,
};

/// What the audit saw for one transition attempt.
#[derive(Clone, Debug, PartialEq)]
pub enum AuditEvent {
    /// The planned change, inspected before anything was constructed.
    Proposed(Vec<(&'static str, NodeAction)>),
    /// A committed change, with the components it displaced in retirement order.
    Committed { displaced: Vec<&'static str> },
    /// A rejected change, which left the active generation untouched.
    Aborted(TransitionOutcome),
}

/// Records every transition. No provider switch affects it, so the one instance keeps its
/// record across reloads.
#[component]
pub struct TransitionAudit {
    #[default]
    events: Mutex<Vec<AuditEvent>>,
}

impl TransitionAudit {
    /// Takes the events recorded since the last call.
    pub fn take(&self) -> Vec<AuditEvent> {
        std::mem::take(&mut *self.events.lock().expect("audit events lock"))
    }

    fn record(&self, event: AuditEvent) {
        self.events.lock().expect("audit events lock").push(event);
    }
}

#[methods]
impl TransitionAudit {
    #[hook(TransitionProposal, always)]
    async fn inspect(&self, pending: PendingTransition) -> TransitionVerdict {
        let plan = pending
            .components()
            .iter()
            .map(|component| (component.component, component.action))
            .collect();

        self.record(AuditEvent::Proposed(plan));

        TransitionVerdict::Accept
    }

    #[hook(TransitionCommitted)]
    async fn committed(&self, report: RuntimeTransitionReport) {
        self.record(AuditEvent::Committed {
            displaced: report.displaced().to_vec(),
        });
    }

    #[hook(TransitionAborted)]
    async fn aborted(&self, report: RuntimeTransitionReport) {
        self.record(AuditEvent::Aborted(report.outcome().clone()));
    }
}
