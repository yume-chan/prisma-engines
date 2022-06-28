use async_trait::async_trait;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use opentelemetry::{
    global, sdk,
    sdk::{
        export::trace::{ExportResult, SpanData, SpanExporter},
        propagation::TraceContextPropagator,
    },
    trace::TracerProvider,
};
use serde_json::json;
use std::fmt::{self, Debug};
use std::{collections::HashMap, time::SystemTime};

/// Pipeline builder
#[derive(Debug)]
pub struct PipelineBuilder {
    trace_config: Option<sdk::trace::Config>,
}

/// Create a new stdout exporter pipeline builder.
pub fn new_pipeline() -> PipelineBuilder {
    PipelineBuilder::default()
}

impl Default for PipelineBuilder {
    /// Return the default pipeline builder.
    fn default() -> Self {
        Self { trace_config: None }
    }
}

impl PipelineBuilder {
    /// Assign the SDK trace configuration.
    pub fn with_trace_config(mut self, config: sdk::trace::Config) -> Self {
        self.trace_config = Some(config);
        self
    }
}

impl PipelineBuilder {
    pub fn install_simple(mut self, log_callback: ThreadsafeFunction<String>) -> sdk::trace::Tracer {
        global::set_text_map_propagator(TraceContextPropagator::new());
        let exporter = ClientSpanExporter::new(log_callback);

        let mut provider_builder = sdk::trace::TracerProvider::builder().with_simple_exporter(exporter);
        if let Some(config) = self.trace_config.take() {
            provider_builder = provider_builder.with_config(config);
        }
        let provider = provider_builder.build();
        let tracer = provider.tracer("opentelemetry");
        let _ = global::set_tracer_provider(provider);

        tracer
    }
}

/// A [`ClientSpanExporter`] that sends spans to the JS callback.
pub struct ClientSpanExporter {
    callback: ThreadsafeFunction<String>,
}

impl ClientSpanExporter {
    pub fn new(callback: ThreadsafeFunction<String>) -> Self {
        Self { callback }
    }
}

impl Debug for ClientSpanExporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientSpanExporter").finish()
    }
}

#[async_trait]
impl SpanExporter for ClientSpanExporter {
    /// Export spans to stdout
    async fn export(&mut self, batch: Vec<SpanData>) -> ExportResult {
        for span in batch {
            let result = span_to_json(&span);
            self.callback.call(Ok(result), ThreadsafeFunctionCallMode::Blocking);
        }

        Ok(())
    }
}

fn span_to_json(span: &SpanData) -> String {
    let attributes: HashMap<String, String> =
        span.attributes
            .iter()
            .fold(HashMap::default(), |mut map, (key, value)| {
                if key.as_str() == "query" {
                    map.insert("query".to_string(), value.to_string());
                }

                map
            });

    let a = json!({
        "span": true,
        "trace_id": format!("{}", span.span_context.trace_id()),
        "span_id": format!("{}",span.span_context.span_id()),
        "parent_span_id": format!("{}",span.parent_span_id),
        "name": span.name,
        "start_time": span.start_time.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis().to_string(),
        "end_time": span.end_time.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis().to_string(),
        "attributes": attributes
    });

    serde_json::to_string(&a).unwrap()
}
