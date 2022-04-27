use super::*;
use darling::FromMeta;
use query_tests_setup::ConnectorTag;

#[derive(Debug, FromMeta)]
pub struct SchemaDriftArgs {
    #[darling(default)]
    pub suite: Option<String>,

    #[darling(default)]
    pub schema_a: Option<SchemaHandler>,

    #[darling(default)]
    pub schema_b: Option<SchemaHandler>,

    #[darling(default)]
    pub only: OnlyConnectorTags,

    #[darling(default)]
    pub exclude: ExcludeConnectorTags,

    #[darling(default)]
    pub capabilities: RunOnlyForCapabilities,
}

impl SchemaDriftArgs {
    pub fn validate(&self, on_module: bool) -> Result<(), darling::Error> {
        validate_suite(&self.suite, on_module)?;

        if self.schema_a.is_none() || self.schema_b.is_none() {
            return Err(darling::Error::custom(
                "A `schema_a` and `schema_b` annotation on the test (`schema_a(handler), schema_b(handler)`) is required.",
            ));
        }

        Ok(())
    }

    /// Returns all the connectors that the test is valid for.
    pub fn connectors_to_test(&self) -> Vec<ConnectorTag> {
        connectors_to_test(&self.only, &self.exclude)
    }
}
