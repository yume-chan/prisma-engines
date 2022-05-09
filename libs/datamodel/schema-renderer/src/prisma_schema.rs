use std::{fmt, ops};

use crate::{CompositeType, CompositeTypeId, FieldId, Index, IndexField, IndexId, Model, ModelId, ScalarField};

#[derive(Default)]
pub struct PrismaSchema<'a> {
    models: Vec<Model<'a>>,
    types: Vec<CompositeType<'a>>,
    model_fields: Vec<(ModelId, ScalarField<'a>)>,
    type_fields: Vec<(CompositeTypeId, ScalarField<'a>)>,
    indices: Vec<(ModelId, Index<'a>)>,
    index_fields: Vec<(IndexId, IndexField<'a>)>,
}

impl<'a> PrismaSchema<'a> {
    pub fn model_id_for_name(&self, name: &str) -> Option<ModelId> {
        self.models.iter().position(|model| model.name() == name).map(ModelId)
    }

    pub fn push_model(&mut self, model: Model<'a>) -> ModelId {
        self.models.push(model);

        ModelId(self.models.len() - 1)
    }

    pub fn push_type(&mut self, r#type: CompositeType<'a>) -> CompositeTypeId {
        self.types.push(r#type);

        CompositeTypeId(self.types.len() - 1)
    }

    pub fn push_model_field(&mut self, model_id: ModelId, field: ScalarField<'a>) -> FieldId {
        self.model_fields.push((model_id, field));

        FieldId(self.model_fields.len() - 1)
    }

    pub fn push_type_field(&mut self, ct_id: CompositeTypeId, field: ScalarField<'a>) -> FieldId {
        self.type_fields.push((ct_id, field));

        FieldId(self.type_fields.len() - 1)
    }

    pub fn push_index(&mut self, model_id: ModelId, index: Index<'a>) -> IndexId {
        self.indices.push((model_id, index));

        IndexId(self.indices.len() - 1)
    }

    pub fn push_index_field(&mut self, index_id: IndexId, field: IndexField<'a>) -> FieldId {
        self.index_fields.push((index_id, field));

        FieldId(self.index_fields.len() - 1)
    }
}

impl<'a> ops::Index<ModelId> for PrismaSchema<'a> {
    type Output = Model<'a>;

    fn index(&self, index: ModelId) -> &Self::Output {
        &self.models[index.0]
    }
}

impl<'a> ops::IndexMut<ModelId> for PrismaSchema<'a> {
    fn index_mut(&mut self, index: ModelId) -> &mut Self::Output {
        &mut self.models[index.0]
    }
}

impl<'a> fmt::Display for PrismaSchema<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (id, r#type) in self.types.iter().enumerate() {
            let id = CompositeTypeId(id);

            f.write_str(r#"type {} {{"#, r#type.name())?;
        }
    }
}
