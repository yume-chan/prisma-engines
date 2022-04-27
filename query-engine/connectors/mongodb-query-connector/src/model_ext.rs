use native_types::MongoDbType;
use prisma_models::ScalarField;

pub(crate) trait ScalarFieldExt {
    fn is_object_id(&self) -> bool;
}

impl ScalarFieldExt for ScalarField {
    fn is_object_id(&self) -> bool {
        if let Some(ref nt) = self.native_type {
            let mongo_type: MongoDbType = nt.deserialize_native_type();
            matches!(mongo_type, MongoDbType::ObjectId)
        } else {
            false
        }
    }
}
