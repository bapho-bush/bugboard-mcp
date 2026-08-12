use std::collections::HashMap;

#[derive(Default, Debug)]
pub(crate) struct HandleStore {
    refs_by_handle: HashMap<String, (HandleKind, String)>,
    handles_by_ref: HashMap<(HandleKind, String), String>,
    next_bug: usize,
    next_project: usize,
}

impl HandleStore {
    pub(crate) fn remember(&mut self, kind: HandleKind, reference: &str) -> String {
        let key = (kind, reference.to_owned());
        if let Some(handle) = self.handles_by_ref.get(&key) {
            return handle.clone();
        }

        let (prefix, counter) = match kind {
            HandleKind::Bug => ("bug", &mut self.next_bug),
            HandleKind::Project => ("project", &mut self.next_project),
        };
        *counter += 1;
        let handle = format!("{prefix}-{counter}");
        self.refs_by_handle
            .insert(handle.clone(), (kind, reference.to_owned()));
        self.handles_by_ref.insert(key, handle.clone());
        handle
    }

    pub(crate) fn resolve(&self, kind: HandleKind, handle: &str) -> Option<String> {
        self.refs_by_handle
            .get(handle)
            .filter(|(actual, _)| *actual == kind)
            .map(|(_, reference)| reference.clone())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum HandleKind {
    Bug,
    Project,
}
