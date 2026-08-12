use std::future::Future;

use serde::Serialize;
use serde_json::json;
use tokio::sync::Mutex;

use crate::errors::ToolFailure;

// ponytail: one bugboard account per process; use keyed locks only if write throughput matters.
static WRITE_LOCK: Mutex<()> = Mutex::const_new(());

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RequestedState {
    Enabled,
    Disabled,
}

impl RequestedState {
    fn as_bool(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

#[derive(Debug, Eq, PartialEq, Serialize)]
pub(crate) struct WriteOutcome {
    pub(crate) requested: bool,
    pub(crate) changed: bool,
    pub(crate) previous_state: bool,
    pub(crate) final_state: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WriteSafety {
    requested: RequestedState,
    previous_state: bool,
}

impl WriteSafety {
    pub(crate) fn new(requested: RequestedState, previous_state: bool) -> Self {
        Self {
            requested,
            previous_state,
        }
    }

    pub(crate) fn needs_mutation(self) -> bool {
        self.requested.as_bool() != self.previous_state
    }

    pub(crate) fn finish(self, final_state: bool) -> Result<WriteOutcome, ToolFailure> {
        let requested = self.requested.as_bool();
        if final_state != requested {
            return Err(ToolFailure::new(
                "write_postcondition_failed",
                "Bugboard did not reach the requested state.",
                json!({
                    "requested": requested,
                    "previous_state": self.previous_state,
                    "final_state": final_state,
                }),
            ));
        }

        Ok(WriteOutcome {
            requested,
            changed: self.previous_state != final_state,
            previous_state: self.previous_state,
            final_state,
        })
    }
}

pub(crate) async fn apply_write<Read, ReadFuture, Mutate, MutateFuture>(
    requested: RequestedState,
    mut read_state: Read,
    mutate: Mutate,
) -> Result<WriteOutcome, ToolFailure>
where
    Read: FnMut() -> ReadFuture,
    ReadFuture: Future<Output = Result<bool, ToolFailure>>,
    Mutate: FnOnce(RequestedState) -> MutateFuture,
    MutateFuture: Future<Output = Result<(), ToolFailure>>,
{
    let _guard = WRITE_LOCK.lock().await;
    let previous_state = read_state().await?;
    let safety = WriteSafety::new(requested, previous_state);
    if !safety.needs_mutation() {
        return safety.finish(previous_state);
    }

    // The request may reach bugboard even when its response is lost or malformed.
    // The authoritative post-state decides success.
    let mutation = mutate(requested).await;
    let final_state = match read_state().await {
        Ok(state) => state,
        Err(read_error) => {
            return match mutation {
                Err(mutation_error) if !mutation_error.mutation_delivery_is_uncertain() => {
                    Err(mutation_error)
                }
                _ => Err(read_error),
            };
        }
    };
    let outcome = safety.finish(final_state);

    match (mutation, outcome) {
        (Ok(()), outcome) => outcome,
        (Err(error), Ok(outcome)) if error.mutation_delivery_is_uncertain() => Ok(outcome),
        (Err(error), Ok(_)) => Err(error),
        (Err(error), Err(_)) if !error.mutation_delivery_is_uncertain() => Err(error),
        (Err(_), Err(postcondition)) => Err(postcondition),
    }
}
