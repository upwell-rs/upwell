use std::time::Instant;

use upwell_config::{ChangedBinding, ComponentHookReport};
use upwell_core::RuntimeGenerationId;

use super::{
    ComponentTransitionReport, PhaseTiming, RuntimeTransitionReport, TransitionAttemptId,
    TransitionFailure, TransitionOutcome, TransitionPhase, TransitionRejection, trace,
};

/// How a transition attempt that ran to completion ended.
pub(crate) enum TransitionCompletion {
    /// Nothing changed; the base generation stays current.
    NoOp,
    /// A new generation was published with the given config generation.
    Committed {
        generation: RuntimeGenerationId,
        config_generation: u64,
    },
}

/// Accumulates one attempt's report as its phases run, then finishes it for every
/// outcome.
///
/// A recorder dropped without [`finish`](Self::finish) belongs to a cancelled attempt; it
/// emits a cancellation event instead of a report, because no caller remains to receive
/// one.
pub(crate) struct TransitionRecorder {
    attempt: TransitionAttemptId,
    base_generation: RuntimeGenerationId,
    config_generation: u64,
    current: Option<(TransitionPhase, Instant)>,
    phases: Vec<PhaseTiming>,
    changed: Box<[ChangedBinding]>,
    components: Box<[ComponentTransitionReport]>,
    hooks: Box<[ComponentHookReport]>,
    finished: bool,
}

impl TransitionRecorder {
    pub(crate) fn begin(
        attempt: TransitionAttemptId,
        base_generation: RuntimeGenerationId,
        config_generation: u64,
    ) -> Self {
        Self {
            attempt,
            base_generation,
            config_generation,
            current: None,
            phases: Vec::new(),
            changed: Box::default(),
            components: Box::default(),
            hooks: Box::default(),
            finished: false,
        }
    }

    /// Ends the running phase, if any, and starts timing `phase`.
    pub(crate) fn enter(&mut self, phase: TransitionPhase) {
        self.close_phase();
        self.current = Some((phase, Instant::now()));
    }

    pub(crate) fn record_changed(&mut self, changed: &[ChangedBinding]) {
        self.changed = changed.into();
    }

    pub(crate) fn record_components(&mut self, components: Box<[ComponentTransitionReport]>) {
        self.components = components;
    }

    pub(crate) fn record_hooks(&mut self, hooks: Vec<ComponentHookReport>) {
        self.hooks = hooks.into_boxed_slice();
    }

    /// Builds the attempt's report, emits its tracing events, and returns it, or wraps it
    /// with the rejecting error in [`crate::Error::TransitionRejected`].
    pub(crate) fn finish(
        mut self,
        result: crate::Result<TransitionCompletion>,
    ) -> crate::Result<RuntimeTransitionReport> {
        let phase = self
            .current
            .map_or(TransitionPhase::Stage, |(phase, _)| phase);

        self.close_phase();
        self.finished = true;

        let (outcome, error) = match result {
            Ok(TransitionCompletion::NoOp) => (TransitionOutcome::NoOp, None),
            Ok(TransitionCompletion::Committed {
                generation,
                config_generation,
            }) => {
                self.config_generation = config_generation;

                (TransitionOutcome::Committed { generation }, None)
            }
            Err(error) => {
                let failure = TransitionFailure::classify(phase, &error);

                (TransitionOutcome::Rejected { phase, failure }, Some(error))
            }
        };
        let report = self.take_report(outcome);

        trace::report(&report);

        match error {
            None => Ok(report),
            Some(error) => Err(crate::Error::TransitionRejected(Box::new(
                TransitionRejection::new(report, error),
            ))),
        }
    }

    fn take_report(&mut self, outcome: TransitionOutcome) -> RuntimeTransitionReport {
        RuntimeTransitionReport {
            attempt: self.attempt,
            base_generation: self.base_generation,
            outcome,
            config_generation: self.config_generation,
            changed: std::mem::take(&mut self.changed),
            components: std::mem::take(&mut self.components),
            hooks: std::mem::take(&mut self.hooks),
            phases: std::mem::take(&mut self.phases).into_boxed_slice(),
        }
    }

    fn close_phase(&mut self) {
        if let Some((phase, started)) = self.current.take() {
            self.phases.push(PhaseTiming {
                phase,
                elapsed: started.elapsed(),
            });
        }
    }
}

impl Drop for TransitionRecorder {
    fn drop(&mut self) {
        if self.finished {
            return;
        }

        let phase = self.current.map(|(phase, _)| phase);

        trace::cancelled(self.attempt, self.base_generation, phase);
    }
}
