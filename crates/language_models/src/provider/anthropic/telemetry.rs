//! Upstream reported editor events (a completion was invoked, accepted or
//! rejected, with the language's name) to an endpoint Anthropic runs for it,
//! signed with the user's own API key. This product sends no telemetry, so
//! the reporter is kept as a type its callers can hold and reports nothing.

use language_model::LanguageModel;

#[derive(Clone, Debug)]
pub struct AnthropicEventData {
    pub completion_type: AnthropicCompletionType,
    pub event: AnthropicEventType,
    pub language_name: Option<String>,
    pub message_id: Option<String>,
}

#[derive(Clone, Debug)]
pub enum AnthropicCompletionType {
    Editor,
    Terminal,
    Panel,
}

#[derive(Clone, Debug)]
pub enum AnthropicEventType {
    Invoked,
    Response,
    Accept,
    Reject,
}

pub fn report_anthropic_event(_model: &LanguageModel, _event: AnthropicEventData, _cx: &gpui::App) {
}

#[derive(Clone)]
pub struct AnthropicEventReporter;

impl AnthropicEventReporter {
    pub fn new(_model: &LanguageModel, _cx: &gpui::App) -> Self {
        Self
    }

    /// Sends nothing: see the module documentation.
    pub fn report(&self, _event: AnthropicEventData) {}
}
