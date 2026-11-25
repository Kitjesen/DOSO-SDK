use crate::{rpc, DosoClient, Error, Result};
use prost_types::value::Kind;

use super::System;

impl System<'_> {
    /// Read local product state and the token required for guarded map switching.
    pub async fn product_status(&self) -> Result<rpc::ProductStatus> {
        let snapshot = DosoClient::read(
            rpc::system_client::SystemClient::new(self.0.channel.clone())
                .product_status(self.0.request(rpc::Empty {}))
                .await,
        )?;
        let fields = snapshot
            .fields
            .ok_or_else(|| Error::protocol("product status has no fields"))?
            .fields;
        let current = match fields.get("current").and_then(|value| value.kind.as_ref()) {
            Some(Kind::StructValue(value)) => &value.fields,
            _ => return Err(Error::protocol("product status has no current state")),
        };
        let available = match fields
            .get("available")
            .and_then(|value| value.kind.as_ref())
        {
            Some(Kind::BoolValue(value)) => Some(*value),
            None | Some(Kind::NullValue(_)) => None,
            _ => return Err(Error::protocol("product availability must be a boolean")),
        };
        let status = match current.get("status").and_then(|value| value.kind.as_ref()) {
            Some(Kind::StringValue(value)) if !value.is_empty() => value.clone(),
            _ => return Err(Error::protocol("product status must be a nonempty string")),
        };
        let mut result = rpc::ProductStatus {
            available,
            status,
            product: None,
            product_session_id: None,
        };
        for (name, target) in [
            ("product", &mut result.product),
            ("product_session_id", &mut result.product_session_id),
        ] {
            *target = match current.get(name).and_then(|value| value.kind.as_ref()) {
                Some(Kind::StringValue(value)) if !value.is_empty() => Some(value.clone()),
                None | Some(Kind::NullValue(_)) => None,
                _ => {
                    return Err(Error::protocol(format!(
                        "product {name} must be a nonempty string"
                    )))
                }
            };
        }
        Ok(result)
    }
}
