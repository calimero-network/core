//! State management for context lifecycle

use std::collections::BTreeMap;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::application::{Application, ApplicationId};

#[derive(Debug)]
pub struct ContextState {
    contexts: BTreeMap<ContextId, Context>,
    applications: BTreeMap<ApplicationId, Application>,
}

impl ContextState {
    pub fn new() -> Self {
        Self {
            contexts: BTreeMap::new(),
            applications: BTreeMap::new(),
        }
    }

    pub fn get_context(&self, context_id: &ContextId) -> Option<&Context> {
        self.contexts.get(context_id)
    }

    pub fn get_application(&self, app_id: &ApplicationId) -> Option<&Application> {
        self.applications.get(app_id)
    }

    pub fn insert_context(&mut self, context: Context) {
        self.contexts.insert(context.id, context);
    }

    pub fn insert_application(&mut self, application: Application) {
        self.applications.insert(application.id, application);
    }
}
